# Text Selection Deep Dive & Implementation Plan

## Problem Statement

Text selection is currently not possible in terminal panes because the TUI application captures ALL mouse events via crossterm's `EnableMouseCapture`, preventing the terminal emulator's native text selection from working.

## Root Cause Analysis

### Current Mouse Capture Behavior

1. **Mouse Capture is Always On by Default**
   - In `main.rs:32-35`, `EnableMouseCapture` is executed on startup
   - The application starts with `mouse_capture_enabled: true` in `app.rs:254`

2. **All Mouse Events Are Consumed**
   - In `main.rs:584-592`, every `Event::Mouse` is dispatched to `app.handle_mouse()`
   - The handler processes: Down, Up, Drag, ScrollUp, ScrollDown events
   - None of these events "fall through" to the terminal emulator

3. **Toggle Exists But Disables Everything**
   - `Ctrl+Shift+M` toggles `mouse_capture_enabled` (`app.rs:462-467`)
   - When disabled, `DisableMouseCapture` is executed (`main.rs:622`)
   - This completely disables mouse interaction: no resizing, no pane swapping, no clicking
   - Help text confirms: "Toggle mouse capture for terminal text selection" (`ui.rs:263`)

### Why This Prevents Text Selection

When `EnableMouseCapture` is active:
- The terminal emulator sends mouse events to the application via escape sequences
- The terminal emulator does NOT perform its own text selection
- The application must implement text selection itself (which it doesn't)

When `DisableMouseCapture` is active:
- The terminal emulator handles mouse events natively
- Text selection works via the terminal emulator
- But ALL application mouse features are lost (resize, swap, click-to-focus, scroll, etc.)

### The Core Issue

The current implementation uses an all-or-nothing approach:
- **All-in**: Application handles everything, no text selection
- **All-out**: Terminal handles everything, no TUI features

## Mouse Event Taxonomy

### Events Currently Handled by the Application

#### Critical (Must Preserve)
1. **Divider Resize** - `MouseEventKind::Drag(MouseButton::Left)` on boundaries
   - Detects resize targets via `resize_target_at()`
   - Drags adjust pane sizes
   - User-facing feature: "Mouse drag: Resize pane dividers"

2. **Pane Swap** - Title bar drag and drop
   - Started by clicking title bar: `MouseEventKind::Down(MouseButton::Left)` on title
   - Dragged: `MouseEventKind::Drag(MouseButton::Left)` 
   - Dropped: `MouseEventKind::Up(MouseButton::Left)` on another pane
   - User-facing feature: "Drag pane title onto another pane: Swap pane positions"

3. **Pane Focus** - Click to focus
   - `MouseEventKind::Down(MouseButton::Left)` in pane content area
   - Essential for keyboard routing

4. **Chrome Interaction** - Buttons and menus
   - Maximize button: `placement.maximize_hit()`
   - Close button: `placement.close_hit()`
   - Settings button: `settings_button_area()`
   - Modal interactions (help, theme, new pane picker, panel settings, close confirm)

#### Semi-Critical (Could Be Optional)
5. **Scroll Wheel** - `MouseEventKind::ScrollUp/Down`
   - Sends mouse wheel events to child process if supported
   - Falls back to scrollback navigation
   - Could be made conditional on capture state

#### Non-Critical (Safe to Ignore)
6. **Middle-click** - Not currently used
7. **Right-click** - Not currently used
8. **Modifier+click** - Not currently used

### Terminal Emulator Native Behaviors

When mouse capture is disabled, terminal emulators typically support:
1. **Left-click drag**: Text selection
2. **Double-click**: Word selection
3. **Triple-click**: Line selection
4. **Shift+click**: Extended selection
5. **Middle-click**: Paste (on X11)
6. **Right-click**: Context menu (sometimes)
7. **Ctrl+click**: Open URLs (if supported)
8. **Scroll wheel**: Scroll terminal buffer

## Implementation Goal

Enable native terminal text selection (via mouse drag) WITHOUT losing critical TUI mouse features:
- ✅ Divider resize via drag
- ✅ Pane swap via title drag
- ✅ Click-to-focus
- ✅ Chrome interaction (buttons, modals)
- ✅ Scroll wheel
- ✅ **NEW**: Text selection in terminal panes

## Technical Challenges

### Challenge 1: Event Conflict
Text selection and divider resize both use `MouseEventKind::Drag(MouseButton::Left)`. We need to determine user intent based on:
- **Start location**: Is the drag starting on a divider boundary or in pane content?
- **Timing**: Did the user click and immediately drag (selection) or pause then drag (resize)?
- **Distance**: Is the drag movement significant enough to indicate resize intent?

### Challenge 2: Focus vs Selection
A left click in pane content should:
- Focus the pane (for keyboard routing)
- Also start a potential text selection (terminal-native)

These are not mutually exclusive if we're careful about event flow.

### Challenge 3: State Management
We need to track:
- When a drag starts (click location and time)
- What the drag context is (resize, swap, selection, or unknown)
- When to hand off to terminal vs handle internally

## Proposed Solution: Hybrid Mouse Capture

### Architecture Overview

Implement a **selective mouse capture** system that:
1. Always keeps mouse capture ENABLED at the crossterm level
2. Internally decides which events to handle vs pass through
3. Uses context-aware logic to determine intent

### Implementation Strategy

#### Option A: Partial Mouse Event Filtering (Recommended)

**Concept**: Only capture and handle mouse events in specific contexts, ignore others to allow terminal-native behavior.

**Implementation Details**:

1. **Track Drag Context**
   ```rust
   enum DragContext {
       None,
       ResizeBoundary { /* ... */ },
       TitleBar { pane_id: usize },
       Content { started_at: Instant, position: (u16, u16) },
   }
   ```

2. **Smart Event Dispatch**
   ```rust
   // On MouseDown:
   // - If on divider boundary → start resize drag (capture)
   // - If on title bar → start swap drag (capture)
   // - If on chrome (buttons) → handle click (capture)
   // - If in pane content → focus pane, DON'T capture, let terminal handle selection
   
   // On MouseDrag:
   // - If resize drag active → handle resize (capture)
   // - If swap drag active → handle swap preview (capture)
   // - If content drag → IGNORE (terminal handles text selection)
   
   // On MouseUp:
   // - If resize/swap drag active → finish operation (capture)
   // - Otherwise → ignore (terminal handles)
   
   // On Scroll:
   // - Always handle (capture) - terminals don't pass scroll events to child processes
   ```

3. **Boundary Detection**
   - Use existing `resize_target_at()` to detect if click is on a divider
   - Expand hit area with tolerance (e.g., 1-2 cells) for usability
   - If click is NOT on boundary/chrome/title, treat as content interaction

**Pros**:
- Terminal-native text selection works automatically
- Minimal code changes
- All critical TUI features preserved
- No custom text selection implementation needed
- Feels natural to users

**Cons**:
- Cannot implement advanced text selection features (copy to clipboard)
- Text selection styling controlled by terminal emulator
- May need to document behavior for users

#### Option B: Full Manual Text Selection

**Concept**: Implement complete text selection system within the TUI.

**Implementation Details**:

1. **Selection State Tracking**
   ```rust
   struct TextSelection {
       pane_id: usize,
       start_row: u16,
       start_col: u16,
       end_row: u16,
       end_col: u16,
       active: bool,
   }
   ```

2. **Content-Based Drag Detection**
   - Track mouse down position and time
   - If drag starts in pane content and moves > threshold → selection mode
   - Render highlight overlay on selected text
   - Add copy-to-clipboard support (Ctrl+C or right-click menu)

3. **Rendering Selection Highlights**
   - Modify `build_styled_view()` to apply inverse style to selected cells
   - Or overlay selection rectangles using ratatui widgets

4. **Copy Support**
   - Extract text from vt100 parser buffer
   - Use clipboard library (e.g., `copypasta` crate)
   - Add keybinding (Ctrl+C when selection active, or Ctrl+Shift+C)

**Pros**:
- Complete control over selection behavior
- Can support copy-to-clipboard
- Consistent UX regardless of terminal emulator
- Can support advanced features (rectangle selection, multi-line)

**Cons**:
- Significant implementation effort
- Need to handle all edge cases (wrapping, scrollback, wide chars)
- Clipboard integration adds dependencies
- May feel different from user's terminal emulator
- Complex rendering logic needed

#### Option C: Context-Sensitive Mouse Protocol Switching

**Concept**: Dynamically enable/disable mouse capture based on cursor location.

**Implementation Details**:

1. **Mouse Mode Zones**
   ```rust
   enum MouseMode {
       Captured,     // App handles events (near boundaries, title bars, chrome)
       Passthrough,  // Terminal handles events (in pane content areas)
   }
   ```

2. **Zone-Based Switching**
   - Track current mouse position each frame
   - If hovering over resize boundary → enable capture, show resize cursor
   - If hovering over pane content → disable capture, allow selection
   - Execute EnableMouseCapture/DisableMouseCapture as needed

3. **Debouncing**
   - Don't toggle on every frame - use hysteresis
   - Only toggle when mode has been stable for N frames
   - Cache last mode to avoid rapid toggling

**Pros**:
- Best of both worlds: TUI features + native selection
- Terminal handles all selection nuances
- App handles all TUI interactions

**Cons**:
- Technical complexity: frequent enable/disable may cause issues
- Potential flickering or lag
- Some terminals may not handle rapid toggling well
- Need to handle state during transitions
- May not work on all terminal emulators

### Recommended Approach: Option A (Partial Event Filtering)

**Rationale**:
1. **Lowest risk**: Minimal changes to existing code
2. **Best compatibility**: Works with all terminal emulators
3. **Fastest implementation**: Can be done in <100 lines of code
4. **Maintains stability**: Doesn't change mouse capture state dynamically
5. **Progressive**: Can evolve to Option B later if needed

## Implementation Plan

### Phase 1: Core Infrastructure (30 minutes)

1. Add `DragContext` enum to track drag state
2. Add timestamp tracking to mouse events
3. Add tolerance constant for boundary detection (e.g., `DRAG_INITIATION_THRESHOLD: u16 = 3`)

### Phase 2: Smart MouseEvent Dispatch (1 hour)

1. Modify `handle_mouse()` in `app.rs`:
   - Track mouse down location and context
   - Distinguish between:
     - Boundary drags (resize)
     - Title drags (swap)
     - Content drags (selection - pass through)
   - Update `DragContext` based on location

2. Implement intent detection:
   ```rust
   fn determine_mouse_intent(&self, mouse: &MouseEvent, size: Rect) -> MouseIntent {
       // Check if on resize boundary
       if self.resize_target_at(size, mouse.column, mouse.row).is_some() {
           return MouseIntent::Resize;
       }
       
       // Check if on title bar
       if let Some(placement) = self.placement_at(size, mouse.column, mouse.row) {
           if placement.title_hit(...) {
               return MouseIntent::Swap;
           }
       }
       
       // Check if on chrome
       if /* on buttons, modals, etc */ {
           return MouseIntent::Chrome;
       }
       
       // Default: content interaction (text selection)
       MouseIntent::Content
   }
   ```

3. Update drag handling:
   - Only start drag tracking for `Resize` and `Swap` intents
   - For `Content`, just focus the pane and let terminal handle it
   - Ignore drag events without active drag context

### Phase 3: Boundary Refinement (30 minutes)

1. Test edge cases:
   - Clicking near pane borders
   - Dragging across pane boundaries
   - Rapid click-drag sequences
   - Title bar edge cases

2. Fine-tune boundary detection:
   - Adjust hit area for resize boundaries (currently uses exact match)
   - Add visual feedback (cursor shape already implemented)
   - Consider "sticky" boundaries that extend beyond visual divider

3. Adjust drag initiation logic:
   - May require minimum movement before confirming resize intent
   - Or require modifier key (e.g., Alt+drag for resize)

### Phase 4: Testing & Polish (1 hour)

1. Test with multiple terminal emulators:
   - Terminal.app (macOS)
   - iTerm2
   - Alacritty
   - Kitty
   - WezTerm
   - GNOME Terminal
   - Windows Terminal

2. Test all TUI features:
   - Divider resize
   - Pane swap
   - Click-to-focus
   - Scroll wheel
   - All chrome interactions
   - Modal interactions

3. Document behavior:
   - Update help text
   - Add README section on text selection

## Code Changes Required

### File: `src/app.rs`

#### Change 1: Add DragContext and MouseIntent enums

```rust
// Add near top with other structs
#[derive(Debug, Clone, Copy)]
enum MouseIntent {
    Resize,
    Swap,
    Chrome,
    Content,
}

#[derive(Debug, Clone, Copy)]
enum DragContext {
    None,
    Resize,
    Swap,
}
```

#### Change 2: Add drag initiation state

```rust
// In DragResize struct, add:
struct DragResize {
    pane_a: usize,
    pane_b: usize,
    direction: Direction,
    last_coord: u16,
    pane_ids: Vec<usize>,
    initiated: bool,  // NEW: tracks if we've confirmed user intent
}

// In DragPaneSwap struct:
struct DragPaneSwap {
    source_pane_id: usize,
    hovered_pane_id: Option<usize>,
    moved: bool,
    initiated: bool,  // NEW
}
```

#### Change 3: Modify handle_mouse()

```rust
pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent, size: Rect) -> anyhow::Result<()> {
    // 1. Handle drag completions first
    if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
        self.finish_active_drag(mouse, size)?;
        return Ok(());
    }

    // 2. Handle ongoing drags
    if self.has_active_drag() {
        if matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left)) {
            self.continue_active_drag(mouse, size)?;
        }
        return Ok(());
    }

    // 3. Determine intent for new interactions
    let intent = self.determine_mouse_intent(&mouse, size);
    
    match intent {
        MouseIntent::Resize => {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.start_resize_drag(mouse, size)?;
            }
        }
        MouseIntent::Swap => {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.start_swap_drag(mouse, size)?;
            }
        }
        MouseIntent::Chrome => {
            self.handle_chrome_click(mouse, size)?;
        }
        MouseIntent::Content => {
            // Focus pane but DON'T start drag tracking
            // This allows terminal to handle text selection
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                if let Some(placement) = self.placement_at(size, mouse.column, mouse.row) {
                    self.focus_pane(placement.pane_id);
                }
            }
            // For scroll events in content, still handle them
            if matches!(mouse.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
                self.handle_scroll(mouse, size)?;
            }
            // For drag events in content area with no active drag, ignore
            // (terminal will handle text selection)
        }
    }

    Ok(())
}
```

#### Change 4: Add intent determination helper

```rust
fn determine_mouse_intent(&self, mouse: &MouseEvent, size: Rect) -> MouseIntent {
    use crossterm::event::MouseEventKind;
    
    // Skip if not a relevant event
    if !matches!(mouse.kind, 
        MouseEventKind::Down(MouseButton::Left) | 
        MouseEventKind::Drag(MouseButton::Left) |
        MouseEventKind::ScrollUp | 
        MouseEventKind::ScrollDown
    ) {
        return MouseIntent::Content;
    }

    // Check chrome areas first (settings button, modals)
    if self.in_modal_or_chrome(mouse.column, mouse.row, size) {
        return MouseIntent::Chrome;
    }

    // Check resize boundaries
    if self.resize_target_at(size, mouse.column, mouse.row).is_some() {
        return MouseIntent::Resize;
    }

    // Check title bars
    if let Some(placement) = self.placement_at(size, mouse.column, mouse.row) {
        if placement.is_title_bar(mouse.row) {
            return MouseIntent::Swap;
        }
    }

    // Default to content (allows text selection)
    MouseIntent::Content
}

fn in_modal_or_chrome(&self, x: u16, y: u16, size: Rect) -> bool {
    // Check if mouse is in any modal or on chrome elements
    if self.modal.is_some() {
        return true; // Always capture when modal is open
    }
    
    // Check settings button
    if contains(settings_button_area(size), x, y) {
        return true;
    }
    
    false
}
```

### File: `src/ui.rs`

#### Minor Update: Clarify help text

```rust
// Currently: "Ctrl+Shift+M: Toggle mouse capture for terminal text selection\n\
// Change to: "Ctrl+Shift+M: Toggle mouse capture (disables pane resize/swap)\n\
// Add new line: "Note: Text selection is always available when clicking in pane content"
```

## Edge Cases to Handle

1. **Drag Starting at Boundary, Moving Into Content**
   - Already handled: drag context is locked once started
   - Will continue resize/swap even if cursor leaves boundary

2. **Quick Clicks Near Boundaries**
   - May need hysteresis or minimum movement threshold
   - Or: require modifier key for resize (Alt+drag)

3. **Title Bar at Pane Edge**
   - Title bar clicks should trigger swap
   - Content clicks just below should allow selection
   - Need clear visual distinction

4. **Scroll Wheel in Selection Mode**
   - Should scroll work during text selection? 
   - Probably yes - terminal should handle this naturally

5. **Modal Open State**
   - When modal is open, should capture all mouse events
   - Prevents accidental text selection while configuring

6. **Keyboard Interaction During Selection**
   - If user is selecting text and presses a key, what happens?
   - Key goes to focused pane (normal behavior)
   - Terminal may cancel selection on key press (terminal behavior)

7. **Copy/Paste**
   - Ctrl+C should send interrupt signal (not copy text)
   - Terminal-specific copy commands (Cmd+C on macOS, Ctrl+Shift+C on Linux)
   - Paste handled by terminal (Ctrl+V, Cmd+V, etc.)

## Testing Checklist

### Manual Testing
- [ ] Select a single word (double-click)
- [ ] Select multiple words (click and drag)
- [ ] Select a full line (triple-click)
- [ ] Select across lines (drag vertically)
- [ ] Resize pane divider
- [ ] Swap panes via drag
- [ ] Click to focus pane
- [ ] Click maximize button
- [ ] Click close button
- [ ] Click settings button
- [ ] Scroll with mouse wheel
- [ ] Use scrollback (Shift+PageUp)
- [ ] Open modals and interact
- [ ] Close modal and select text

### Platform Testing
- [ ] macOS Terminal.app
- [ ] macOS iTerm2
- [ ] Linux GNOME Terminal
- [ ] Linux Alacritty/Kitty
- [ ] Windows Terminal
- [ ] VSCode integrated terminal (if applicable)

## Alternative Considerations

### If Text Selection Is Not Working

1. **Check terminal emulator support**: Some terminals may not support native selection when mouse reporting is enabled, even if we're not consuming drag events
   
2. **Try Option C**: Dynamic mouse capture toggling
   - Enable capture only when hovering boundaries
   - Disable when hovering content
   - More complex but may work better with certain terminals

3. **Implement Option B**: Full manual selection
   - More work but complete control
   - Necessary if terminals don't cooperate

### If Resize/Swap Breaks

1. **Add visual feedback**: Highlight boundaries when hovering
   - Use existing pointer shape system
   - Maybe add divider highlight

2. **Require modifier key**: Alt+drag for resize, Shift+drag for swap
   - Prevents accidental drags
   - Makes text selection more reliable
   - Document in help text

3. **Increase boundary hit area**: Make resize boundaries "stickier"
   - Easier to grab dividers
   - May help with precise mouse control

## Conclusion

**Recommended Implementation**: Option A (Partial Event Filtering)

This approach:
- ✅ Enables native text selection with minimal code changes
- ✅ Preserves all critical TUI features
- ✅ Maintains stability of existing mouse interactions
- ✅ Works with any terminal emulator
- ✅ Can be implemented in ~2-3 hours
- ✅ Low risk of breaking existing functionality

The key insight is that we don't need to disable mouse capture - we just need to selectively ignore certain events (content drags) and let the terminal handle them naturally while still capturing the events we need for TUI functionality.
