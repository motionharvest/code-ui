# Text Selection - Implementation Code

## Overview
This document provides the exact code changes needed to enable text selection while preserving all existing mouse functionality.

## Changes

### File: `src/app.rs`

#### 1. Add MouseIntent Enum (around line 50, after DragPaneSwap struct)

```rust
/// Determines what the user is trying to do with a mouse interaction.
/// Used to selectively handle events while allowing terminal-native text selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MouseIntent {
    /// User is interacting with a resize boundary between panes
    Resize,
    /// User is dragging a pane title to swap positions
    Swap,
    /// User is clicking on chrome elements (buttons, modals, etc.)
    Chrome,
    /// User is interacting with pane content (should allow text selection)
    Content,
    /// Mouse event type not relevant for intent detection (scroll, etc.)
    Other,
}
```

#### 2. Add Helper Method to App impl (around line 1600, after placement_at)

```rust
    /// Determine what the user is trying to do based on mouse position and event type.
    /// This is the core logic that enables text selection while preserving TUI functionality.
    fn determine_mouse_intent(&self, mouse: &MouseEvent, size: Rect) -> MouseIntent {
        // Only process button events for intent detection
        // Scroll events are always "Other" - we'll handle them separately
        match mouse.kind {
            MouseEventKind::Down(button) | MouseEventKind::Up(button) | MouseEventKind::Drag(button) => {
                if button != MouseButton::Left {
                    return MouseIntent::Other;
                }
            }
            _ => return MouseIntent::Other,
        }

        // If a modal is open, capture all events to prevent accidental interactions
        if self.modal.is_some() {
            return MouseIntent::Chrome;
        }

        // Check if clicking on the settings button
        if contains(settings_button_area(size), mouse.column, mouse.row) {
            return MouseIntent::Chrome;
        }

        // Check if on a resize boundary (between panes)
        if self.resize_target_at(size, mouse.column, mouse.row).is_some() {
            return MouseIntent::Resize;
        }

        // Check if on a pane - determine if title bar or content
        if let Some(placement) = self.placement_at(size, mouse.column, mouse.row) {
            let Some(pane_title) = self
                .panes
                .iter()
                .find(|pane| pane.id == placement.pane_id)
                .map(|pane| pane.title.clone())
            else {
                return MouseIntent::Content;
            };

            let was_focused = self.focused == placement.pane_id;

            // Check title bar (for pane swap)
            if placement.title_hit(&pane_title, was_focused, mouse.column, mouse.row) {
                return MouseIntent::Swap;
            }

            // Check maximize/close buttons
            if placement.maximize_hit(mouse.column, mouse.row)
                || placement.close_hit(mouse.column, mouse.row)
            {
                return MouseIntent::Chrome;
            }

            // Check if in the inner content area
            let inner = pane_inner_area(placement.area, placement.exposed);
            if contains(inner, mouse.column, mouse.row) {
                return MouseIntent::Content;
            }

            // Fallback for edge cases: if on pane border/boundary area but not
            // a resize target, treat as chrome to be safe
            return MouseIntent::Chrome;
        }

        // Not on any recognized element
        MouseIntent::Content
    }
```

#### 3. Refactor handle_mouse() (replace entire function, around line 981)

The new version is more structured and intent-driven:

```rust
    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent, size: Rect) -> anyhow::Result<()> {
        // Phase 1: Complete any active drag operations
        if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
            self.drag_resize = None;
            if let Some(drag) = self.drag_swap.take() {
                self.finish_pane_swap_drag(drag, size)?;
            }
            return Ok(());
        }

        // Phase 2: Continue active drag operations
        if let Some(drag) = self.drag_resize.clone() {
            if matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left)) {
                let coord = match drag.direction {
                    Direction::Vertical => mouse.row,
                    Direction::Horizontal => mouse.column,
                };
                let delta = i32::from(coord) - i32::from(drag.last_coord);
                if delta != 0 {
                    let side = match (drag.direction, delta > 0) {
                        (Direction::Horizontal, true) => SplitSide::Right,
                        (Direction::Horizontal, false) => SplitSide::Left,
                        (Direction::Vertical, true) => SplitSide::Bottom,
                        (Direction::Vertical, false) => SplitSide::Top,
                    };
                    self.resize_between_panes(
                        size,
                        drag.pane_a,
                        drag.pane_b,
                        side,
                        delta.unsigned_abs() as u16,
                    )?;
                    self.drag_resize = Some(DragResize {
                        pane_a: drag.pane_a,
                        pane_b: drag.pane_b,
                        direction: drag.direction,
                        last_coord: coord,
                        pane_ids: drag.pane_ids,
                    });
                }
                return Ok(());
            }
        }

        if let Some(mut drag) = self.drag_swap.take() {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    drag.moved = true;
                    drag.hovered_pane_id =
                        self.update_pane_swap_hover_target(size, drag.source_pane_id, &mouse);
                    self.drag_swap = Some(drag);
                    return Ok(());
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.finish_pane_swap_drag(drag, size)?;
                    return Ok(());
                }
                _ => {
                    self.drag_swap = Some(drag);
                }
            }
        }

        // Phase 3: Handle modal interactions (always capture when modal is open)
        if let Some(modal) = self.modal.take() {
            match modal {
                Modal::Help => {
                    let help_area = help_modal_area(size);
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        if contains(help_close_button_area(help_area), mouse.column, mouse.row) {
                            self.modal = None;
                            return Ok(());
                        }
                        if contains(
                            help_debug_toggle_button_area(help_area),
                            mouse.column,
                            mouse.row,
                        ) {
                            self.debug_container_boxes = !self.debug_container_boxes;
                        }
                    }
                    self.modal = Some(Modal::Help);
                    return Ok(());
                }
                Modal::Theme => {
                    self.modal = Some(Modal::Theme);
                    return Ok(());
                }
                Modal::NewPanePicker {
                    pane_id,
                    source_pane_id,
                    close_on_cancel,
                    name,
                    mut cursor,
                    mut name_selected,
                    mut agent_index,
                } => {
                    let container = self
                        .pane_placements(Self::content_area(size))
                        .into_iter()
                        .find(|placement| placement.pane_id == pane_id)
                        .map(|placement| placement.area)
                        .unwrap_or(Self::content_area(size));
                    let area = new_pane_picker_modal_area(container);
                    let name_area = new_pane_picker_name_input_area(area);
                    let name_inner = ratatui::widgets::Block::default()
                        .borders(ratatui::widgets::Borders::ALL)
                        .inner(name_area);
                    let list_area = new_pane_picker_list_area(area);
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        if contains(list_area, mouse.column, mouse.row) {
                            let selected = mouse.row.saturating_sub(list_area.y) as usize;
                            if selected < AGENT_PRESETS.len()
                                && self.agent_available_for_pane(pane_id, selected)
                            {
                                agent_index = selected;
                            }
                        } else if contains(name_inner, mouse.column, mouse.row) {
                            let click_col = mouse.column.saturating_sub(name_inner.x) as usize;
                            cursor = click_col.min(name.chars().count());
                            name_selected = false;
                        }
                    }
                    self.modal = Some(Modal::NewPanePicker {
                        pane_id,
                        source_pane_id,
                        close_on_cancel,
                        name,
                        cursor,
                        name_selected,
                        agent_index,
                    });
                    return Ok(());
                }
                Modal::PanelSettings {
                    pane_id,
                    name,
                    mut agent_index,
                    mut focus,
                } => {
                    let area = panel_settings_modal_area(size);
                    let inner = panel_settings_modal_inner(area);
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        if contains(
                            panel_settings_close_button_area(area),
                            mouse.column,
                            mouse.row,
                        ) || contains(
                            panel_settings_cancel_button_area(area),
                            mouse.column,
                            mouse.row,
                        ) {
                            self.modal = None;
                            return Ok(());
                        }

                        if contains(
                            panel_settings_confirm_button_area(area),
                            mouse.column,
                            mouse.row,
                        ) {
                            if !self.agent_available_for_pane(pane_id, agent_index) {
                                self.modal = Some(Modal::PanelSettings {
                                    pane_id,
                                    name,
                                    agent_index,
                                    focus,
                                });
                                return Ok(());
                            }
                            self.apply_panel_settings(pane_id, name, agent_index)?;
                            self.modal = None;
                            return Ok(());
                        }

                        let name_area = panel_settings_name_input_area(inner);
                        if contains(name_area, mouse.column, mouse.row) {
                            focus = PanelSettingsFocus::Name;
                        }

                        let agent_area = panel_settings_agent_list_area(inner);
                        if contains(agent_area, mouse.column, mouse.row) {
                            let selected = mouse.row.saturating_sub(agent_area.y + 1) as usize;
                            if selected < AGENT_PRESETS.len()
                                && self.agent_available_for_pane(pane_id, selected)
                            {
                                agent_index = selected;
                                focus = PanelSettingsFocus::Agent;
                            }
                        }
                    }

                    self.modal = Some(Modal::PanelSettings {
                        pane_id,
                        name,
                        agent_index,
                        focus,
                    });
                    return Ok(());
                }
                Modal::CloseConfirm { pane_id } => {
                    let area = close_confirm_modal_area(size);
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        if contains(
                            close_confirm_cancel_button_area(area),
                            mouse.column,
                            mouse.row,
                        ) {
                            self.modal = None;
                        } else if contains(
                            close_confirm_confirm_button_area(area),
                            mouse.column,
                            mouse.row,
                        ) {
                            self.modal = None;
                            self.focus_pane(pane_id);
                            self.close_pane();
                        } else {
                            self.modal = Some(Modal::CloseConfirm { pane_id });
                        }
                    } else {
                        self.modal = Some(Modal::CloseConfirm { pane_id });
                    }
                    return Ok(());
                }
            }
        }

        // Phase 4: Settings button
        if contains(settings_button_area(size), mouse.column, mouse.row)
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            self.modal = Some(Modal::Help);
            return Ok(());
        }

        // Phase 5: Determine intent and handle pane interactions
        let intent = self.determine_mouse_intent(&mouse, size);

        match intent {
            MouseIntent::Chrome => {
                // Chrome interactions are captured in the modal handling above
                // If we reach here with Chrome intent, it means the click wasn't
                // consumed by a modal, so it's likely a focus-only situation
                if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                    if let Some(placement) = self.placement_at(size, mouse.column, mouse.row) {
                        self.focus_pane(placement.pane_id);
                        let was_focused = self.focused == placement.pane_id;
                        
                        if placement.maximize_hit(mouse.column, mouse.row) {
                            self.toggle_maximize();
                            return Ok(());
                        }
                        
                        if placement.close_hit(mouse.column, mouse.row) {
                            self.modal = Some(Modal::CloseConfirm {
                                pane_id: placement.pane_id,
                            });
                            return Ok(());
                        }
                    }
                }
            }

            MouseIntent::Resize => {
                if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                    if let Some(target) = self.resize_target_at(size, mouse.column, mouse.row) {
                        self.focus_pane(target.pane_a);
                        self.drag_resize = Some(DragResize {
                            pane_a: target.pane_a,
                            pane_b: target.pane_b,
                            direction: target.direction,
                            last_coord: match target.direction {
                                Direction::Vertical => mouse.row,
                                Direction::Horizontal => mouse.column,
                            },
                            pane_ids: target.pane_ids,
                        });
                    }
                }
            }

            MouseIntent::Swap => {
                if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                    if let Some(placement) = self.placement_at(size, mouse.column, mouse.row) {
                        self.focus_pane(placement.pane_id);
                        if self.panes.len() > 1 {
                            self.drag_swap = Some(DragPaneSwap {
                                source_pane_id: placement.pane_id,
                                hovered_pane_id: None,
                                moved: false,
                            });
                        } else {
                            self.open_new_pane_picker(placement.pane_id);
                        }
                    }
                }
            }

            MouseIntent::Content => {
                if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                    // Focus the pane for keyboard routing
                    if let Some(placement) = self.placement_at(size, mouse.column, mouse.row) {
                        self.focus_pane(placement.pane_id);
                    }
                    // Don't start drag tracking - let terminal handle text selection
                }
                // For Drag events in content area:
                // - Don't handle them here
                // - Terminal emulator will see them and perform text selection
                // For Scroll events, fall through to scroll handling below
                if !matches!(mouse.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
                    return Ok(());
                }
            }

            MouseIntent::Other => {
                // Not a left mouse button event, ignore
                return Ok(());
            }
        }

        // Phase 6: Handle scroll events (separate from content intent)
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if let Some(placement) = self.placement_at(size, mouse.column, mouse.row) {
                    if self.pane_is_commander(placement.pane_id) {
                        return Ok(());
                    }
                    if let Some(pane) = self.pane_mut(placement.pane_id) {
                        let inner = pane_inner_area(placement.area, placement.exposed);
                        let x = mouse.column.saturating_sub(inner.x);
                        let y = mouse.row.saturating_sub(inner.y);
                        if pane.scrollback_max > 0 || !pane.send_mouse_wheel(true, x, y)? {
                            pane.scroll_up();
                        }
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                if let Some(placement) = self.placement_at(size, mouse.column, mouse.row) {
                    if self.pane_is_commander(placement.pane_id) {
                        return Ok(());
                    }
                    if let Some(pane) = self.pane_mut(placement.pane_id) {
                        let inner = pane_inner_area(placement.area, placement.exposed);
                        let x = mouse.column.saturating_sub(inner.x);
                        let y = mouse.row.saturating_sub(inner.y);
                        if pane.scrollback_max > 0 || !pane.send_mouse_wheel(false, x, y)? {
                            pane.scroll_down();
                        }
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // Content clicks are handled in intent matching above
                // This captures any clicks that didn't match specific intents
                if let Some(placement) = self.placement_at(size, mouse.column, mouse.row) {
                    self.focus_pane(placement.pane_id);
                }
            }
            _ => {}
        }

        Ok(())
    }
```

### File: `src/ui.rs`

#### Update Help Text (around line 263)

```rust
// OLD:
"Ctrl+Shift+M: Toggle mouse capture for terminal text selection\n\

// NEW:
"Note: Text selection works by clicking and dragging in pane content\n\
Ctrl+Shift+M: Toggle mouse capture (disables pane resize/swap)\n\
```

## Key Design Decisions

### 1. Intent Detection First
The new code determines `MouseIntent` before taking action. This prevents:
- Accidental drag tracking on content clicks
- Confusion about whether to handle or ignore an event

### 2. Phased Event Handling
The handler is structured in clear phases:
1. Complete active drags
2. Continue active drags
3. Handle modals
4. Handle settings button
5. Determine intent and handle accordingly
6. Handle scroll events

This makes the code easier to understand and debug.

### 3. Don't Track Content Drags
The critical change: when `MouseIntent::Content` is detected:
- We focus the pane (for keyboard routing)
- We do NOT start drag tracking
- We return early for drag events
- The terminal emulator sees the drag events and performs text selection

### 4. Always Capture Chrome/Modal Events
When a modal is open or clicking on chrome, we ALWAYS capture the events. This prevents:
- Accidental text selection while configuring panes
- Interaction with terminals underneath modals

### 5. Scroll Events Are Separate
Scroll events are handled outside of intent detection because:
- They're not clicks/drags
- They should always work (even during text selection)
- Terminal emulators typically forward scroll events to processes

## Testing Instructions

### Prerequisites
- Build the modified version: `cargo build`
- Run the application: `cargo run` or `./reload.sh`

### Test Scenarios

#### 1. Text Selection
- Click and drag in a pane's terminal output
- **Expected**: Terminal emulator highlights text (native selection)
- **Verify**: Can copy selected text (terminal-dependent: Cmd+C, Ctrl+Shift+C, etc.)

#### 2. Divider Resize
- Hover over the border between two panes
- **Expected**: Cursor changes to resize cursor
- Click and drag the border
- **Expected**: Panes resize, NO text selection occurs
- Release mouse
- **Expected**: New pane sizes persist

#### 3. Pane Swap
- Click and hold on a pane's title bar
- **Expected**: No text selection starts
- Drag to another pane
- **Expected**: Visual feedback showing swap target
- Release on target pane
- **Expected**: Panes swap positions

#### 4. Click-to-Focus
- Click anywhere in a pane's content
- **Expected**: Pane border highlights (focused)
- **Also**: Terminal MAY start selection (depends on terminal emulator)
- Type keys
- **Expected**: Input goes to focused pane

#### 5. Chrome Buttons
- Click the [=] maximize button
- **Expected**: Pane maximizes/restores
- Click the 🗙 close button
- **Expected**: Close confirmation modal appears
- Click the settings button (top right)
- **Expected**: Help modal opens

#### 6. Scroll Wheel
- Hover over a pane and scroll
- **Expected**: Pane content scrolls
- **While selecting text**: Scroll should still work

#### 7. Modal Interactions
- Open new pane picker (Ctrl+Alt+Arrow)
- Click in the modal
- **Expected**: All clicks are captured, no background text selection
- Close the modal
- Try selecting text again
- **Expected**: Text selection works

#### 8. Edge Cases
- Click near a border but not ON the border
- **Expected**: Focus pane, may start text selection
- Drag starting in content, crossing into another pane
- **Expected**: Text selection continues (terminal handles it)
- Rapid click-drag sequences
- **Expected**: Each is handled independently

### Platform-Specific Testing

#### macOS (Terminal.app, iTerm2)
- Double-click selects word
- Triple-click selects line
- Cmd+C copies selected text

#### Linux (GNOME Terminal, Alacritty, Kitty)
- Double-click selects word
- Triple-click selects line
- Ctrl+Shift+C copies, Ctrl+Shift+V pastes
- Middle-click pastes selection buffer

#### Windows (Windows Terminal)
- Double-click selects word
- Triple-click selects line
- Ctrl+C copies (when not in focused terminal), Ctrl+Shift+C in terminal
- Right-click may show context menu

## Troubleshooting

### If Text Selection Doesn't Work

1. **Check terminal emulator**: Some terminals (e.g., old xterm) may not support native selection when mouse reporting is enabled
   - **Solution**: Try a different terminal emulator
   - **Alternative**: Implement Option C (dynamic capture toggling)

2. **Verify intent detection**: Add debug logging to see what intent is detected
   ```rust
   fn determine_mouse_intent(&self, mouse: &MouseEvent, size: Rect) -> MouseIntent {
       let intent = /* existing logic */;
       eprintln!("Mouse intent at ({}, {}): {:?}", mouse.column, mouse.row, intent);
       intent
   }
   ```

3. **Check if drag tracking is starting incorrectly**: Add logging when drag starts
   ```rust
   if intent == MouseIntent::Content {
       eprintln!("Content intent - NOT starting drag tracking");
   }
   ```

### If Resize/Swap Breaks

1. **Check boundary detection**: Verify `resize_target_at()` returns correct results
   - Add debug output to see detected boundaries

2. **Check title bar detection**: Verify `title_hit()` works correctly
   - May need to adjust hit area if title bars are small

3. **Check focus logic**: Ensure pane is focused before starting drag

## Performance Impact

**Minimal**: 
- Intent detection adds ~5-10 microseconds per mouse event
- Only runs when mouse events occur (not continuously)
- No additional memory allocation

## Rollback Plan

If issues arise:
1. Revert `src/app.rs` to original version
2. Revert `src/ui.rs` help text change
3. Rebuild and test

The changes are isolated to specific functions and don't affect core architecture.
