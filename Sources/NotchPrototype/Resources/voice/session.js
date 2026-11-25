/* Kai's private WebRTC media document. Authentication stays in App Server. */
(() => {
  'use strict';
  let current = null;
  const post = (session, type, fields = {}) => {
    if (session && current !== session) return;
    const envelope = {type, ...fields};
    if (session) envelope.sessionId = session.id;
    window.webkit.messageHandlers.kaiVoice.postMessage(envelope);
  };
  function dispose(session) {
    if (!session) return;
    clearTimeout(session.disconnectTimer);
    clearInterval(session.meterTimer);
    clearInterval(session.statsTimer);
    clearTimeout(session.resumeTimer);
    if (session.promptSource) {session.promptSource.onended = null; try {session.promptSource.stop();} catch (_) {}}
    session.promptDestination?.stream.getTracks().forEach(track => track.stop());
    session.initialBuffer = null;
    session.stream?.getTracks().forEach(track => track.stop());
    session.peer?.close();
    session.audio?.pause();
    session.audio?.remove();
    if (session.audio) session.audio.srcObject = null;
    session.context?.close().catch(() => {});
  }
  function stop() {
    const previous = current;
    current = null;
    dispose(previous);
  }
  const failure = (session, message) => {
    if (current !== session) return;
    post(session, 'error', {message});
    stop();
  };
  const activity = (session, value) => {
    if (current !== session || session.activity === value) return;
    session.activity = value; post(session, 'activity', {activity: value});
  };
  async function start(id, captureMicrophone, initialPrompt = '', promptOnly = false) {
    stop();
    const session = {id, captureMicrophone, promptOnly, active: false, announced: false, initialized: false};
    current = session;
    try {
      const peer = session.peer = new RTCPeerConnection();
      const transceiver = session.transceiver = peer.addTransceiver('audio', {direction: 'sendrecv'});
      const audio = session.audio = document.createElement('audio');
      audio.autoplay = true; audio.muted = true; audio.hidden = true;
      document.body.appendChild(audio);
      if (!captureMicrophone) {
        const button = session.button = document.createElement('button');
        button.textContent = 'Start test audio'; button.disabled = true;
        button.style.cssText = 'font:16px system-ui;padding:12px;margin:24px';
        button.onclick = () => session.context?.resume().catch(error => failure(session, String(error)));
        document.body.appendChild(button);
      }
      const channel = session.channel = peer.createDataChannel('oai-events');
      const announce = () => {
        if (current !== session || session.announced || peer.connectionState !== 'connected' || channel.readyState !== 'open' || !session.initialized) return;
        session.announced = true;
        audio.muted = !session.active || session.promptOnly;
        post(session, 'connected');
      };
      channel.onopen = announce;
      channel.onclose = () => failure(session, 'Codex voice data connection closed');
      channel.onerror = () => failure(session, 'Codex voice data connection failed');
      channel.onmessage = event => {
        if (current !== session || typeof event.data !== 'string' || event.data.length > 262144) return;
        try {
          const value = JSON.parse(event.data);
          if (value.type === 'input_audio_buffer.speech_started') activity(session, 'listening');
          if (value.type === 'input_audio_buffer.speech_stopped') activity(session, 'thinking');
          if (value.type === 'output_audio_buffer.started') activity(session, 'speaking');
          if (value.type === 'output_audio_buffer.stopped') activity(session, 'listening');
          if (value.type === 'session.started' || value.type === 'session.updated') {session.initialized = true; announce();}
          if (value.type === 'session.closed') failure(session, 'Codex voice session closed' + (typeof value.reason === 'string' ? ': ' + value.reason.slice(0, 200) : ''));
          else if (value.type === 'error') failure(session, String(value.error?.message || 'Codex voice service error').slice(0, 300));
          // Only diagnostics receive event types. Transcripts are authoritative
          // on App Server's same-thread sideband and are never duplicated here.
          else post(session, 'event', {eventType: value.type});
        } catch (_) { /* Ignore non-JSON data-channel payloads. */ }
      };
      peer.ontrack = event => {
        if (current !== session) return;
        audio.srcObject = event.streams[0] || new MediaStream([event.track]);
        if (session.context) {
          session.remoteAnalyser = session.context.createAnalyser(); session.remoteAnalyser.fftSize = 256;
          session.context.createMediaStreamSource(audio.srcObject).connect(session.remoteAnalyser);
        }
        audio.play().catch(() => failure(session, 'Codex voice audio playback was blocked'));
      };
      peer.onconnectionstatechange = () => {
        if (current !== session) return;
        clearTimeout(session.disconnectTimer);
        if (peer.connectionState === 'failed' || peer.connectionState === 'closed') failure(session, 'Codex voice media connection failed');
        else if (peer.connectionState === 'disconnected') session.disconnectTimer = setTimeout(() => failure(session, 'Codex voice media connection disconnected'), 8000);
        else announce();
      };
      if (!captureMicrophone || promptOnly) session.statsTimer = setInterval(async () => {
        if (current !== session) return;
        try {
          const stats = await peer.getStats();
          let bytes = 0;
          stats.forEach(report => { if (report.type === 'inbound-rtp' && (report.kind === 'audio' || report.mediaType === 'audio')) bytes += report.bytesReceived || 0; });
          if (bytes) post(session, 'stats', {audioBytes: bytes});
        } catch (_) {}
      }, 500);
      if (captureMicrophone) {
        const stream = await navigator.mediaDevices.getUserMedia({audio: {echoCancellation: true, noiseSuppression: true, autoGainControl: true}, video: false});
        if (current !== session) { stream.getTracks().forEach(track => track.stop()); return; }
        session.stream = stream;
        const track = stream.getAudioTracks()[0];
        if (!track) throw new Error('No microphone track is available');
        track.enabled = false;
        track.onended = () => failure(session, 'Microphone disconnected; start voice again');
        await transceiver.sender.replaceTrack(track);
        if (current !== session) return;
        const context = session.context = new AudioContext();
        const analyser = context.createAnalyser(); analyser.fftSize = 256;
        context.createMediaStreamSource(stream).connect(analyser);
        const samples = new Float32Array(analyser.fftSize);
        if (initialPrompt) {
          if (initialPrompt.length > 32 * 1024 * 1024) throw new Error('Recorded voice prompt is too large');
          const bytes = Uint8Array.from(atob(initialPrompt), character => character.charCodeAt(0));
          session.initialBuffer = await context.decodeAudioData(bytes.buffer);
          if (current !== session) return;
        }
        context.resume().catch(() => {});
        session.meterTimer = setInterval(() => {
          if (current !== session || !session.active) return;
          analyser.getFloatTimeDomainData(samples);
          const level = Math.min(1, Math.sqrt(samples.reduce((sum, value) => sum + value * value, 0) / samples.length) * 12);
          let remoteLevel = 0;
          if (session.remoteAnalyser) {
            session.remoteAnalyser.getFloatTimeDomainData(samples);
            remoteLevel = Math.min(1, Math.sqrt(samples.reduce((sum, value) => sum + value * value, 0) / samples.length) * 12);
          }
          const now = Date.now();
          if (remoteLevel > .08) {session.lastOutput = now; activity(session, 'speaking');}
          else if (session.activity === 'speaking' && now - session.lastOutput > 500) activity(session, 'listening');
          if (!session.promptSource && track.enabled && level > .12 && remoteLevel < .08) {
            session.lastInput = now; session.userSpeaking = true; activity(session, 'listening');
          } else if (session.userSpeaking && now - session.lastInput > 600) {
            session.userSpeaking = false; activity(session, 'thinking');
          }
          post(session, 'level', {level: Math.max(track.enabled ? level : 0, remoteLevel)});
        }, 80);
      }
      await peer.setLocalDescription(await peer.createOffer());
      if (current === session) post(session, 'offer', {sdp: peer.localDescription.sdp});
    } catch (error) {
      failure(session, String(error?.message || error).slice(0, 300));
    }
  }
  async function answer(id, sdp) {
    const session = current;
    if (!session || session.id !== id) return;
    try {
      await session.peer.setRemoteDescription({type: 'answer', sdp});
      if (current === session) post(session, 'answerApplied');
    } catch (error) { failure(session, String(error?.message || error).slice(0, 300)); }
  }
  async function activate(id) {
    const session = current;
    if (!session || session.id !== id || session.active) return;
    session.active = true;
    session.audio.muted = session.promptOnly;
    if (!session.initialBuffer) {
      session.stream?.getAudioTracks().forEach(track => { track.enabled = !session.promptOnly; });
      activity(session, 'listening'); return;
    }
    try {
      activity(session, 'thinking');
      const context = session.context;
      const destination = session.promptDestination = context.createMediaStreamDestination();
      await session.transceiver.sender.replaceTrack(destination.stream.getAudioTracks()[0]);
      if (current !== session) return;
      // The held recording goes only to the WebRTC sender, never local speakers.
      session.resumeTimer = setTimeout(() => failure(session, 'Voice audio could not resume; hold the shortcut to try again'), 8000);
      await context.resume();
      if (current !== session) return;
      clearTimeout(session.resumeTimer);
      const source = session.promptSource = context.createBufferSource();
      source.buffer = session.initialBuffer; session.initialBuffer = null;
      source.connect(destination);
      source.onended = async () => {
        if (current !== session) return;
        try {
          const microphone = session.stream.getAudioTracks()[0];
          await session.transceiver.sender.replaceTrack(microphone);
          if (current !== session) return;
          microphone.enabled = !session.promptOnly; session.promptSource = null;
          destination.stream.getTracks().forEach(track => track.stop()); session.promptDestination = null;
          activity(session, 'thinking'); post(session, 'promptSent');
        } catch (error) {failure(session, String(error?.message || error).slice(0, 300));}
      };
      source.start();
    } catch (error) {failure(session, String(error?.message || error).slice(0, 300));}
  }
  async function diagnosticAudio(id, encoded) {
    const session = current;
    if (!session || session.id !== id || session.captureMicrophone) return;
    try {
      post(session, 'event', {eventType: 'diagnosticAudioReceived'});
      const context = session.context = new AudioContext();
      const bytes = Uint8Array.from(atob(encoded), character => character.charCodeAt(0));
      const buffer = await context.decodeAudioData(bytes.buffer);
      post(session, 'event', {eventType: 'diagnosticAudioDecoded'});
      if (current !== session) { context.close().catch(() => {}); return; }
      const destination = context.createMediaStreamDestination();
      session.stream = destination.stream;
      await session.transceiver.sender.replaceTrack(destination.stream.getAudioTracks()[0]);
      if (current !== session) return;
      post(session, 'event', {eventType: 'diagnosticAudioTrackAttached:' + context.state});
      session.button.disabled = false;
      await context.resume();
      const source = context.createBufferSource();
      source.buffer = buffer; source.connect(destination); source.start();
      post(session, 'event', {eventType: 'diagnosticAudioStarted'});
    } catch (error) { failure(session, String(error?.message || error).slice(0, 300)); }
  }
  window.kaiVoice = {start, answer, activate, stop, diagnosticAudio};
  window.addEventListener('pagehide', stop);
  post(null, 'ready');
})();
