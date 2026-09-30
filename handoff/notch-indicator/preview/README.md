# Listening and thinking SVG preview

Open `index.html` in a browser. It starts with a deterministic voice-like demo,
then blends into thinking after 4.4 seconds. “Use microphone” switches the same
four SVG bars to live Web Audio frequency bands. “Blend into thinking” preserves
their current geometry and morphs them over 900 ms into four continuously
sequencing thinking dots. Dot size is deliberately weighted from smaller on the
left to larger on the right.

The artwork is intentionally independent of the notch prototype. The SVG uses
four stable rounded `<rect>` nodes, allowing a later implementation to replace
the silver gradient with any pet profile without changing the motion system.
