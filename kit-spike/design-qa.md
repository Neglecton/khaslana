# Design QA

- Source visual truth: `../docs/assets/gpui-kit-research/pencil/a4JtW.png` and Pencil frame `a4JtW` in `pencil-new.pen`
- Implementation: `kit-spike` native GPUI window
- Viewport: 1440 × 1060 logical pixels
- Source capture: 2882 × 2122 pixels; Pencil frame: 1440 × 1060 logical pixels; approximately 2× export density with outer antialias padding
- Implementation capture: unavailable
- State: wide window, staged files visible

## Full-view comparison evidence

The source visual was opened and its frame geometry was also read directly from Pencil. The rebuilt native window could not be captured: Windows Graphics Capture returned `SetIsBorderRequired failed: 不支持此接口 (0x80004002)` for the custom borderless GPUI window. A post-fix full-view comparison therefore cannot be completed reliably.

## Focused region comparison evidence

Blocked for the same capture failure. The intended focused regions were the toolbar command buttons, the 24px outer window corners, the 24px toolbar-to-workspace gap, and the bottom corners of the navigation/file/commit panels.

## Findings and comparison history

### Earlier P1/P2 findings

- P1: The application shell rendered as a rectangular window instead of the Pencil frame's 24px clipped outer radius.
- P1: After the transparent shell fix, the bottom outer corners were rounded but the toolbar child background still painted square top corners because GPUI's overflow mask is rectangular.
- P2: A one-pixel white line remained at the very top from the Windows DWM non-client border on the resizable window.
- P2: Toolbar command buttons lacked a visible boundary against the toolbar background.
- P2: The workspace started directly below the 60px toolbar instead of at y=84.
- P2: Child content was not clipped by several rounded panel shells, hiding bottom radii.

### Fixes applied

- Added a transparent native window background, transparent unbordered Kit root, and a 24px rounded/clipped workbench shell.
- Added explicit 24px top-left and top-right radii to the toolbar surface so its own background cannot square off the outer top corners.
- Disabled only the Windows DWM border color with `DWMWA_COLOR_NONE`, retaining native resize behavior and the compact-layout test path.
- Added a 1px soft border and contact shadow to toolbar command buttons.
- Matched the Pencil vertical rhythm: toolbar 60px; workspace y=84, h=918; status y=1022, h=14; bottom inset 24px.
- Added overflow clipping to expanded/collapsed navigation, file panels, and commit panel.

### Post-fix evidence

- `cargo check`: passed.
- `cargo build`: passed.
- Visual capture: blocked by the Windows capture API error above. Build results are not treated as visual proof.

## Required fidelity surfaces

- Fonts and typography: not re-evaluated in this iteration; no typography changes were made.
- Spacing and layout rhythm: code geometry now matches the Pencil frame values, but post-fix visual confirmation is blocked.
- Colors and visual tokens: Pencil toolbar/background colors were preserved; button separation was added through the existing soft-border and control-shadow tokens.
- Image quality and assets: no raster imagery is used in this screen; existing component icons were preserved.
- Copy and content: unchanged in this iteration.

## Implementation checklist

- Manually inspect the four outer window corners on Windows.
- Confirm the toolbar command outlines remain visible at 100% and 125% scaling.
- Confirm all panel bottom corners remain rounded in staged, unstaged, and collapsed-sidebar states.
- Capture a 1440 × 1060 post-fix screenshot when a compatible capture route is available, then repeat the side-by-side comparison.

final result: blocked
