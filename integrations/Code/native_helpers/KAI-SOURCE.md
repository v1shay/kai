Code's lazy conversational voice bridge adapts the user's own Kai implementation:
https://github.com/v1shay/kai

Source files inspected: Sources/NotchPrototype/WebRTCVoiceMedia.swift,
Sources/NotchPrototype/Resources/voice/session.js, and kai_voice.py in the local
Kai checkout (/Users/vishayagarwal/Documents/ChatGPT/notchdex).

The user expressly requested reuse of this implementation. The source checkout
has no separate LICENSE file; this attribution does not assert an upstream open
source license. The media document and thin Apple framework shim preserve Kai's
WebRTC workflow. All application state, provider protocol, authentication ownership,
and conversation UI remain in Rust. No Codex token or API key enters the media view.
