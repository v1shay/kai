//! Lazy native WebRTC media adapter. The dylib contains only the Apple media
//! shim; it receives neither Codex credentials nor application transcript data.
use anyhow::{Result, anyhow, ensure};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{Value, json};
use std::{
    ffi::{CStr, CString, c_char, c_void},
    marker::PhantomData,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Listening,
    Thinking,
    Speaking,
}
#[derive(Debug, Clone, PartialEq)]
pub enum MediaEvent {
    Offer {
        session_id: String,
        sdp: String,
    },
    Connected {
        session_id: String,
    },
    Activity {
        session_id: String,
        activity: Activity,
    },
    Error {
        session_id: String,
        message: String,
    },
}
struct CallbackState {
    session: String,
    events: SyncSender<MediaEvent>,
    wake: async_channel::Sender<()>,
    levels: Arc<[AtomicU32; 4]>,
    error: Mutex<Option<String>>,
}
type Callback = unsafe extern "C" fn(*mut c_void, *const c_char);
struct Api {
    create: unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_void, Callback) -> *mut c_void,
    start: unsafe extern "C" fn(*mut c_void, *const c_char),
    command: unsafe extern "C" fn(*mut c_void, *const c_char),
    destroy: unsafe extern "C" fn(*mut c_void),
}
fn api() -> Result<&'static Api> {
    static API: OnceLock<std::result::Result<Api, String>> = OnceLock::new();
    API.get_or_init(||unsafe{
        let executable=std::env::current_exe().map_err(|e|e.to_string())?;
        let bundled=executable.parent().and_then(|p|p.parent()).map(|p|p.join("Resources/libCodeVoice.dylib"));
        let path=bundled.filter(|p|p.is_file()).unwrap_or_else(||PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dist/Code.app/Contents/Resources/libCodeVoice.dylib"));
        if !path.is_file(){return Err("This build is missing Code's native voice media bridge. Rebuild the app bundle to enable conversation voice.".into());}
        let path=CString::new(path.as_os_str().as_encoded_bytes()).map_err(|e|e.to_string())?;
        let library=libc::dlopen(path.as_ptr(),libc::RTLD_NOW|libc::RTLD_LOCAL);
        if library.is_null(){return Err("Could not load Code's native WebRTC media bridge.".into());}
        let symbol=|name:&CStr|->std::result::Result<*mut c_void,String>{let p=libc::dlsym(library,name.as_ptr());if p.is_null(){Err("The native voice bridge has an incompatible version.".into())}else{Ok(p)}};
        // Keep the tiny platform dylib loaded for app lifetime: WebKit can finish
        // asynchronous delegate retirement after a media object is destroyed.
        Ok(Api{create:std::mem::transmute::<*mut c_void,unsafe extern "C" fn(*mut c_void,*const c_char,*mut c_void,Callback)->*mut c_void>(symbol(c"code_voice_create")?),start:std::mem::transmute::<*mut c_void,unsafe extern "C" fn(*mut c_void,*const c_char)>(symbol(c"code_voice_start")?),command:std::mem::transmute::<*mut c_void,unsafe extern "C" fn(*mut c_void,*const c_char)>(symbol(c"code_voice_command")?),destroy:std::mem::transmute::<*mut c_void,unsafe extern "C" fn(*mut c_void)>(symbol(c"code_voice_destroy")?)})
    }).as_ref().map_err(|e|anyhow!(e.clone()))
}
unsafe extern "C" fn callback(context: *mut c_void, text: *const c_char) {
    if context.is_null() || text.is_null() {
        return;
    }
    // The boxed state outlives the native object; destruction retires its weak
    // delegates and permission callbacks before Rust releases this allocation.
    let state = unsafe { &*context.cast::<CallbackState>() };
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    if bytes.len() > 256 * 1024 {
        return;
    }
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return;
    };
    if value["sessionId"] != state.session {
        return;
    }
    if std::env::var("KAI_CODE_VOICE_DIAGNOSTIC").as_deref()==Ok("1"){
        match value["type"].as_str(){
            Some("diagnostic")=>eprintln!("Kai media stage: {} status={} visible={} unoccluded={} allowed={}",value["step"].as_str().unwrap_or(""),value["status"],value["hostVisible"],value["unoccluded"],value["allowed"]),
            Some("offer"|"connected"|"promptSent")=>eprintln!("Kai media event: {}",value["type"].as_str().unwrap_or("")),
            Some("event") if ["microphoneRequested","microphoneGranted","preparingOffer"].contains(&value["eventType"].as_str().unwrap_or(""))=>eprintln!("Kai media stage: {}",value["eventType"].as_str().unwrap_or("")),
            _=>{}
        }
    }
    let event = match value["type"].as_str() {
        Some("level") => {
            if let Some(level) = value["level"].as_f64().filter(|n| n.is_finite()) {
                let level = (level as f32).clamp(0., 1.);
                for (i, gain) in [0.82, 0.92, 1., 0.88].iter().enumerate() {
                    state.levels[i].store((level * gain).to_bits(), Ordering::Relaxed);
                }
            }
            let _ = state.wake.try_send(());
            return;
        }
        Some("offer") => {
            let Some(sdp) = value["sdp"].as_str().filter(|s| crate::voice::valid_sdp(s)) else {
                return;
            };
            MediaEvent::Offer {
                session_id: state.session.clone(),
                sdp: sdp.into(),
            }
        }
        Some("connected") => MediaEvent::Connected {
            session_id: state.session.clone(),
        },
        Some("activity") => {
            let activity = match value["activity"].as_str() {
                Some("listening") => Activity::Listening,
                Some("thinking") => Activity::Thinking,
                Some("speaking") => Activity::Speaking,
                _ => return,
            };
            MediaEvent::Activity {
                session_id: state.session.clone(),
                activity,
            }
        }
        Some("error") => MediaEvent::Error {
            session_id: state.session.clone(),
            message: value["message"]
                .as_str()
                .unwrap_or("The voice media connection failed.")
                .chars()
                .take(1000)
                .collect(),
        },
        _ => return,
    };
    if state.events.try_send(event).is_err() {
        *state.error.lock().unwrap_or_else(|e| e.into_inner()) =
            Some("Voice signaling could not keep up. Start voice again.".into());
    }
    let _ = state.wake.try_send(());
}
/// UI-thread-owned media lifetime. Dropping stops all tracks, speaker output,
/// peer/data connections, timers, and the private WKWebView immediately.
pub struct Media {
    native: *mut c_void,
    api: &'static Api,
    state: Box<CallbackState>,
    events: Receiver<MediaEvent>,
    pub wake: async_channel::Receiver<()>,
    pub levels: Arc<[AtomicU32; 4]>,
    _main_thread: PhantomData<Rc<()>>,
}
impl Media {
    pub fn start(window: &gpui::Window, session_id: &str) -> Result<Self> {
        ensure!(
            uuid::Uuid::parse_str(session_id).is_ok(),
            "Invalid voice session identity"
        );
        let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window)
            .map_err(|_| anyhow!("The conversation window is unavailable"))?
            .as_raw()
        else {
            return Err(anyhow!("Conversational voice requires macOS"));
        };
        let api = api()?;
        let (tx, events) = mpsc::sync_channel(16);
        let (wake_tx, wake) = async_channel::bounded(1);
        let levels = Arc::new(std::array::from_fn(|_| AtomicU32::new(0f32.to_bits())));
        let mut state = Box::new(CallbackState {
            session: session_id.into(),
            events: tx,
            wake: wake_tx,
            levels: levels.clone(),
            error: Mutex::new(None),
        });
        let script = CString::new(include_str!("../assets/voice/session.js"))?;
        let id = CString::new(session_id)?;
        let native = unsafe {
            (api.create)(
                if crate::unified::active(){crate::unified::voice_host().ok_or_else(||anyhow!("The voice surface is not visible"))?}else{handle.ns_view.as_ptr()},
                script.as_ptr(),
                (&mut *state as *mut CallbackState).cast(),
                callback,
            )
        };
        ensure!(
            !native.is_null(),
            "The native voice bridge must start on the app's UI thread"
        );
        let media = Self {
            native,
            api,
            state,
            events,
            wake,
            levels,
            _main_thread: PhantomData,
        };
        unsafe {
            (api.start)(media.native, id.as_ptr());
        }
        Ok(media)
    }
    pub fn try_recv(&self) -> std::result::Result<MediaEvent, TryRecvError> {
        match self.events.try_recv() {
            Err(TryRecvError::Empty) => {
                if let Some(message) = self
                    .state
                    .error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
                {
                    Ok(MediaEvent::Error {
                        session_id: self.state.session.clone(),
                        message,
                    })
                } else {
                    Err(TryRecvError::Empty)
                }
            }
            event => event,
        }
    }
    fn command(&self, mut value: Value) -> Result<()> {
        ensure!(!self.native.is_null(), "Voice media has stopped");
        value["sessionId"] = json!(self.state.session);
        let text = CString::new(value.to_string())?;
        unsafe {
            (self.api.command)(self.native, text.as_ptr());
        }
        Ok(())
    }
    pub fn initial_prompt(&self, prompt: &str) -> Result<()> {
        ensure!(
            prompt.len() <= 32 * 1024 * 1024,
            "Recorded prompt is too large"
        );
        self.command(json!({"type":"initialPrompt","audio":prompt}))
    }
    pub fn answer(&self, sdp: &str) -> Result<()> {
        ensure!(
            crate::voice::valid_sdp(sdp),
            "Codex returned an invalid WebRTC answer"
        );
        self.command(json!({"type":"answer","sdp":sdp}))
    }
    pub fn activate(&self) -> Result<()> {
        self.command(json!({"type":"activate"}))
    }
    pub fn mute(&self, muted: bool) -> Result<()> {
        self.command(json!({"type":"mute","muted":muted}))
    }
    pub fn interrupt(&self) -> Result<()> {
        self.command(json!({"type":"interrupt"}))
    }
    pub fn stop(&mut self) {
        if !self.native.is_null() {
            unsafe {
                (self.api.destroy)(self.native);
            }
            self.native = std::ptr::null_mut();
        }
        for level in self.levels.iter() {
            level.store(0f32.to_bits(), Ordering::Relaxed);
        }
    }
}
impl Drop for Media {
    fn drop(&mut self) {
        self.stop();
    }
}
