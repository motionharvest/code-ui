# Text Selection Implementation - Executive Summary

## The Problem
Text selection doesn't work because the app captures ALL mouse events, preventing the terminal emulator from handling text selection natively.

## The Solution
**Partial Event Filtering**: Keep mouse capture enabled, but intelligently decide which events to handle vs. ignore.

### Core Concept
- **Resize drags** → Handle (capture)
- **Swap drags** → Handle (capture)
- **Chrome clicks** → Handle (capture)
- **Content drags** → Ignore (let terminal handle text selection)
- **Content clicks** → Focus pane only (let terminal start selection)

## Implementation Steps

### 1. Add Mouse Intent Detection (~30 lines)
Determine what the user is trying to do based on click location:
- Near divider boundary? → Resize
- On title bar? → Swap
- On buttons/modals? → Chrome
- In pane content? → Content (text selection)

### 2. Track Drag Context (~20 lines)
Only start tracking drags for resize/swap operations. Don't track content drags.

### 3. Update Event Handler (~50 lines)
Modify `handle_mouse()` to:
- Check intent first
- Only start drag tracking for resize/swap
- Focus pane on content click (no drag tracking)
- Ignore content drag events (terminal handles selection)

### 4. Test & Polish (~1 hour)
Verify all features work:
- ✅ Text selection (new!)
- ✅ Divider resize
- ✅ Pane swap
- ✅ Click-to-focus
- All other existing features

## Files to Change
- `src/app.rs` - Core logic (3 functions, ~100 lines)
- `src/ui.rs` - Update help text (1 line)

## Risk Assessment
- **Low Risk**: Minimal changes to existing code
- **High Compatibility**: Works with any terminal emulator
- **Easy Rollback**: Simple to undo if issues arise
- **No New Dependencies**: Uses existing infrastructure

## Timeline
- **Coding**: ~2 hours
- **Testing**: ~1 hour
- **Total**: ~3 hours

## Why This Works
We're not disabling mouse capture - we're just being smarter about which events we consume. When a drag starts in pane content, we:
1. Focus the pane (for keyboard routing)
2. Don't start tracking the drag
3. Let the terminal emulator see the drag events and handle text selection

Meanwhile, drags on boundaries, title bars, and chrome are still captured and handled by the app.

## Alternative Approaches (Not Recommended)
1. **Full manual selection** - Too much work (~500+ lines), not needed
2. **Dynamic capture toggling** - Risky, may cause flickering
3. **Modifier keys required** - Hurts UX, unnecessary

## Next Steps
Ready to implement when approved. Start with Phase 1 (infrastructure) and progress through testing.
