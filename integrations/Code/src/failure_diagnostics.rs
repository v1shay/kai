//! Preserve stack evidence without recording prompts, answers or panic payloads.
pub fn install(){
    if std::env::args().any(|a|a=="--unified-smoke-test"){return;}
    let previous=std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info|{
        if let Some(home)=dirs::home_dir(){
            let folder=home.join("Library/Application Support/Kai Unified/Diagnostics");
            if std::fs::create_dir_all(&folder).is_ok(){
                let report=format!("Kai native runtime panic\nLocation: {}\n{}\n",info.location().map(|v|v.to_string()).unwrap_or_default(),std::backtrace::Backtrace::force_capture());
                let _=std::fs::write(folder.join("last-runtime-panic.txt"),report);
            }
        }
        previous(info);
    }));
}
