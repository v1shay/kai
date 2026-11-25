Based on MIT madeye/rv revision 87055e205fa88803213337846b4d2f59a7a525ee, crates/rv-session (source and tests preserved).

Kai retains the zeroized DH-derived ARD authentication key for the lifetime of the connection and encodes native encrypted key events instead of sending plain VNC key events after Mac-account authentication. ARD mouse right/middle button bits are translated at the same wire boundary. Non-ARD connections are unchanged. No new input service, host process, or proxy is shipped.

Wire-format reference: https://github.com/peetinc/noVNC-ARD/blob/main/ard/ard-patch.js (EncryptedEvent and macOS mouse ordering). Protocol layouts only; implementation uses the transport's existing OpenSSL dependency.

The client keeps the pinned RFB 3.8 negotiation and registry vnc-rs decoder. Native 003.889 mode and its additional server messages are intentionally not enabled. The live Studio validation accepted a click and the exact fixture text with this minimal path.
