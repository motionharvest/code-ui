# Text Selection Implementation Checklist

## Quick Reference

### Goal
Enable native terminal text selection while preserving:
- ✅ Pane resize via drag
- ✅ Pane swap via drag
- ✅ Click-to-focus
- ✅ Scroll wheel
- ✅ Chrome interactions

### Strategy
**Selective Event Handling**: Determine user intent based on click location, only track drags for TUI features, let terminal handle content drags.

---

## Implementation Steps

### Step 1: Add MouseIntent Enum
**Location**: `src/app.rs`, after `DragPaneSwap` struct (~line 50)
**Lines**: ~15

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MouseIntent {
    Resize,
    Swap,
    Chrome,
    Content,
    Other,
}
```

### Step 2: Add determine_mouse_intent() Method
**Location**: `src/app.rs`, in `impl App` block (~line 1600)
**Lines**: ~75

Key logic:
1. Check if modal is open → Chrome
2. Check resize boundaries → Resize
3. Check title bars → Swap
4. Check buttons → Chrome
5. Default → Content

### Step 3: Refactor handle_mouse() Method
**Location**: `src/app.rs`, replace entire function (~line 981)
**Lines**: ~350 (replaces ~450)

Key changes:
1. Add intent detection phase
2. Separate drag tracking for Resize/Swap only
3. For Content intent: focus pane but don't track drag
4. Keep modal/chrome handling unchanged

### Step 4: Update Help Text
**Location**: `src/ui.rs`, line ~263
**Lines**: 2

```rust
// Before:
"Ctrl+Shift+M: Toggle mouse capture for terminal text selection\n\

// After:
"Note: Text selection works by clicking and dragging in pane content\n\
Ctrl+Shift+M: Toggle mouse capture (disables pane resize/swap)\n\
```

---

## Code Locations

| Component | File | Line | Action |
|-----------|------|------|--------|
| MouseIntent enum | `src/app.rs` | ~50 | Add |
| determine_mouse_intent() | `src/app.rs` | ~1600 | Add |
| handle_mouse() | `src/app.rs` | ~981 | Replace |
| Help text | `src/ui.rs` | ~263 | Modify |

---

## Test Plan

### Must Pass (Critical)
- [ ] Text selection works (click & drag in pane content)
- [ ] Pane resize works (drag divider)
- [ ] Pane swap works (drag title bar)
- [ ] Click-to-focus works
- [ ] Scroll wheel works
- [ ] Ctrl+Shift+M toggles mouse capture

### Should Pass (Important)
- [ ] Maximize button works
- [ ] Close button works
- [ ] Settings button works
- [ ] Modals capture mouse correctly
- [ ] No text selection during modal interactions

### Nice to Pass (Edge Cases)
- [ ] Works in multiple terminal emulators
- [ ] Rapid click-drag sequences handled correctly
- [ ] Drag across pane boundaries works correctly
- [ ] No flickering or visual glitches

---

## Build & Test Commands

```bash
# Build
cargo build

# Run
cargo run

# Or use reload script
./reload.sh
```

---

## Expected Behavior by Mouse Location

| Click Location | Intent | Action |
|----------------|--------|--------|
| Divider boundary | Resize | Track drag, resize panes |
| Title bar | Swap | Track drag, swap panes |
| Pane content | Content | Focus pane, terminal handles selection |
| Maximize button | Chrome | Toggle maximize |
| Close button | Chrome | Open close modal |
| Settings button | Chrome | Open help modal |
| Modal area | Chrome | Handle modal interaction |

---

## Rollback Plan

If implementation causes issues:

```bash
# Option 1: Git revert
git diff src/app.rs src/ui.rs > changes.patch
git checkout src/app.rs src/ui.rs
cargo build

# Option 2: Manual revert
# Keep backup of changes.patch
# Restore original files
# Rebuild
```

---

## Time Estimate

- **Step 1-2** (Add types & helper): 30 minutes
- **Step 3** (Refactor handler): 1 hour
- **Step 4** (Update text): 5 minutes
- **Testing**: 1 hour
- **Total**: ~3 hours

---

## Success Criteria

✅ Users can select text in terminal panes (native terminal behavior)
✅ All existing TUI mouse features continue to work
✅ No regressions in keyboard interactions
✅ No visual glitches or flickering
✅ Works across different terminal emulators

---

## Notes

1. **This does NOT disable mouse capture** - we keep capture enabled but selectively ignore content drags
2. **Text selection is terminal-native** - we don't implement selection logic ourselves
3. **Copy/paste is terminal-dependent** - each terminal has its own shortcuts
4. **Ctrl+C still sends interrupt** - not copy (terminal-specific copy required)

---

## Questions?

See full documentation:
- **Why this approach**: `TEXT_SELECTION_ANALYSIS.md`
- **Detailed code changes**: `IMPLEMENTATION_DETAILS.md`
- **Executive summary**: `IMPLEMENTATION_SUMMARY.md`
