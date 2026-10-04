Experimental Kai controls
=========================

The chat header now provides native model and reasoning menus. Models and their
supported efforts come from Codex `model/list`; the selected settings apply to
the next turn (an active turn is not restarted). Model gradients affect only the
labels, leaving pet and application colors intact.

Hold Option for contextual dictation. Context is captured on key-down before Kai
opens, and dictation begins after a short chord-disambiguation delay. Release to
finish, then send normally. Option-Control, Command, and Function shortcuts remain
available. The new standalone chat receives context as background text and image
input. Screenshots are the default. Choose “Prefer lightweight Option context”
in the menu bar, or “Context: screen/light” in the chat controls, to use selected
accessible text when sufficient and fall back to a screenshot otherwise.

macOS Accessibility enables structured context, Screen Recording enables capture,
and microphone/speech permissions enable dictation. Missing screenshot access
falls back to available structured context. Capture files are temporary and follow
the application's existing attachment cleanup lifecycle.

Native back/forward controls follow chats selected through Kai's sidebar.
ApplicationContext is independent of dictation, navigation, and Codex transport,
so later computer-use integrations can reuse the capture boundary.

Verification uses `swift test --scratch-path /tmp/kai-experimental-build` and
`python3 -m unittest test_kai test_kai_experimental`. This does not install, launch,
replace, or restart the running Kai app. Live hotkey and permission UX verification
requires a later launch of this build at the user's discretion.
