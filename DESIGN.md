# SimpleConvert

Native Windows utility in Rust / egui. The window contains two format selectors
and one drop area. Conversion starts on drop. No tabs, sidebar, cards, branding,
extra buttons, illustrations, or decorative animation.

Use installed Segoe UI with egui fallback; 15 px controls, 16 px drop text.
Canvas #FBFBFA, surface #F7F6F3, text #2F3437, boundary #85827B,
focus #415649, error #9F2F2D. Outer margin 20 px; controls 36 px tall.
Corners: selectors 6 px, drop area 8 px. No shadows or gradients.
Keep visible focus, labelled selectors, click/keyboard alternatives to dropping,
and inline progress/result text. No layout movement during state changes.

Launcher icon: off-white circle → arrow → pale-green square on a deep-green
rounded tile. Keep the original vector in assets/icon.svg; ship nine ICO sizes
and a 256 px PNG for the window icon. Embed the ICO in the Windows executable.

Applied references supplied by the user:
- Taste Skill: minimalist-ui and redesign-existing-projects profiles.
  https://github.com/Leonxlnx/taste-skill
- UI/UX Pro Max: verified Minimalism & Swiss Style result; dragging alternative,
  keyboard focus and contrast guidance. Marketing-page recommendations excluded.
  https://github.com/nextlevelbuilder/ui-ux-pro-max-skill
- Awesome Claude Design: Cal.com neutral utility direction from the catalogue.
  https://github.com/VoltAgent/awesome-claude-design
- Design Motion Principles: Emil's productivity/frequency gate. Instant feedback
  avoids decorative motion and remains usable with reduced-motion preferences.
  https://github.com/kylezantos/design-motion-principles
