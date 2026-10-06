Experimental Kai controls
=========================

The chat header now provides native model and reasoning menus. Models and their
supported efforts come from Codex `model/list`; the selected settings apply to
the next turn (an active turn is not restarted). Model gradients affect only the
labels, leaving pet and application colors intact.

Hold Option for contextual dictation. Context is captured on key-down before Kai
opens, and dictation begins after a short chord-disambiguation delay. Release to
finish; Option sends the final transcript automatically by default. Command and
Function still require Send by default. Use the menu bar’s “Dictation auto-send”
submenu to toggle each shortcut independently or enable/disable all. Preferences
persist across launches. Empty dictation never auto-sends, and closing Kai’s
chat cancels auto-send. Option waits for context attachment before sending. Option-Control, Command, and Function shortcuts remain
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

Resizable expanded notch
-----------------------
The existing larger expanded size (physical notch width + 160 points, 320 points
high) remains the initial default. “Default notch size” in the menu bar offers
independent width/height sliders, “Use current size as default,” “Restore saved
default,” and “Reset to original larger size.” Saved defaults survive relaunch.
Dragging changes the current session size; explicitly save it to change the default.

At the bottom of the expanded chat, drag the center grip to change height, the
quarter-width grips to change width symmetrically, or either corner to change both.
The panel remains centered under the physical notch and anchored to the screen top.
Sizes are constrained to the display. A shared native container scales typography,
layer controls, native text fields, attachments, and hit coordinates uniformly.
Independent aspect changes add conversation space rather than stretching content.
Existing path morphs, pet animation, gradients, and control transitions are retained.
