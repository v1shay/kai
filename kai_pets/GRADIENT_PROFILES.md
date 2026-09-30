# Kai pet gradient profiles

`gradient_profiles.json` contains a visual identity and six reusable gradient
recipes for every pet in `animation_catalog.json`. Colors are extracted from
the pet's neutral sprite frame in sRGB, so status UI can match the selected pet
without sampling pixels at runtime.

`gradient_profiles_preview.png` shows the original 22 palettes. The JSON also
contains profiles for the eight newly imported pets.

## Profile contract

Look up `profileByPetID[selectedPetID]`. Each profile provides:

- `palette`: `shadow`, `primary`, `secondary`, `accent`, `highlight`, and a
  contrast-safe `foreground` color.
- `gradients.ambient`: slow background glow or inactive decoration.
- `gradients.thinking`: looping shimmer for inference or deliberation.
- `gradients.working`: faster directional motion for active work.
- `gradients.success`, `warning`, and `error`: pet-aware semantic states. The
  semantic hue is blended into the pet palette instead of replacing it.
- Every gradient includes `angleDegrees`, `cycleDurationMs`, and ordered stops
  with normalized `location` values.

The generated JSON is the runtime asset. Re-run `python3
kai_pets/build_gradient_profiles.py` whenever a sprite sheet or pet is added.

## Core Animation mapping

For a `CAGradientLayer`, map the stop colors to `colors`, locations to
`locations`, and convert `angleDegrees` into unit-space `startPoint` and
`endPoint`. Animate the `locations` array over `cycleDurationMs`; reverse or
wrap the animation to avoid a visible jump. Use the profile's `foreground`
for text and symbols drawn over the gradient.
