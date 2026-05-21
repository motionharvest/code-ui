# Performance Improvement Opportunities

## Analysis Overview

This document identifies performance improvement opportunities in the split_tui TUI application, prioritized by impact-to-risk ratio and ease of implementation.

**Current State:**
- Total lines of code: ~8,200 lines across 8 source files
- Build time: <100ms (excellent)
- Runtime: Terminal-based UI with PTY management, event polling (30ms), and frame rendering
- Architecture: Event-driven with dirty-flag rendering, per-pane PTY threads

## Priority Assessment Criteria

1. **Impact**: How much will this improve performance?
2. **Risk**: Likelihood of introducing bugs or breaking functionality
3. **Effort**: Implementation complexity
4. **Scope**: How often the code path is executed

---

## Priority 1: Lowest Risk, High Impact (Quick Wins)

### 1.1 Eliminate Unnecessary Clone on Mouse Drag Events

**Location:** `src/app.rs:1005`

**Current Code:**
```rust
if let Some(drag) = self.drag_resize.clone() {
```

**Issue:** Every mouse drag event clones the entire `DragResize` struct, including the `pane_ids` vector. This happens frequently during resize operations.

**Recommendation:** Use `as_ref()` instead since we're only reading the data:
```rust
if let Some(drag) = self.drag_resize.as_ref() {
```

**Impact:** High - mouse drags happen frequently during resize
**Risk:** Minimal - just changing borrow semantics, no ownership transfer
**Effort:** 2 minutes
**Scope:** Every mouse drag event during pane resize

---

### 1.2 Pre-compute Pane Lookup Map

**Location:** `src/app.rs`, multiple locations

**Current Pattern:** Repeated linear scans to find panes by ID:
```rust
self.panes.iter().position(|pane| pane.id == pane_id)
self.panes.iter().find(|pane| pane.id == pane_id)
```

**Issue:** O(n) lookup happens frequently:
- 5+ times in `app.rs` per mouse event
- Every frame in `main.rs:255` for focused pane lookup
- Multiple times in modal rendering

**Recommendation:** Add a HashMap<usize, usize> field to App for pane_id → index mapping. Update it when panes are added/removed/created.

**Impact:** Medium - reduces render/event loop overhead
**Risk:** Low - add invariant to maintain map consistency (update on pane creation/removal)
**Effort:** 30 minutes
**Scope:** Every frame render and mouse event

---

### 1.3 Reuse Allocations in Modal Rendering

**Location:** `src/app.rs:1091, 1582` and `src/main.rs:255, 524`

**Current Pattern:**
```rust
.pane_placements(Self::content_area(size))
.into_iter()
.find(|placement| placement.pane_id == pane_id)
.map(|placement| placement.area)
```

**Issue:** Every modal render reconstructs the full placement vector then searches it, even though placements only change on resize.

**Recommendation:** Cache placements on App and invalidate only on resize. Replace with direct hash map lookup for single pane queries.

**Impact:** Medium - modal rendering is less frequent but still per-frame when open
**Risk:** Low - just caching with proper invalidation
**Effort:** 20 minutes
**Scope:** Modal rendering (when modals are open)

---

## Priority 2: Medium Risk, Good Impact

### 2.1 Optimize Mouse Event Routing

**Location:** `src/app.rs`, `handle_mouse()` function

**Current Pattern:** Multiple calls to `placement_at()` in single mouse event handler:
- Once to check if on modal
- Once to check resize boundary
- Once to check pane title
- Once to check content area

**Issue:** Each `placement_at()` call traverses the layout tree recursively.

**Recommendation:** Call `placement_at()` once per event and reuse the result. The `determine_mouse_intent()` function (from IMPLEMENTATION_DETAILS.md) already does this - ensure it's being used.

**Impact:** Medium - reduces CPU per mouse event
**Risk:** Medium - requires careful refactoring of handle_mouse flow
**Effort:** 1 hour
**Scope:** Every mouse event

---

### 2.2 Reduce String Allocations in Commander

**Location:** `src/app.rs:1682-1816`

**Current Pattern:** Excessive string cloning in command processing:
```rust
let user_input = self.commander.input.trim().to_string();
.push(format!("You: {}", user_input.clone()));
```

**Issue:** Unnecessary string allocations on every command submission.

**Recommendation:** 
- Use string references where possible
- Reuse String buffers
- Format directly without intermediate clones

**Impact:** Low-Medium (only affects Commander usage)
**Risk:** Low - localized changes
**Effort:** 30 minutes
**Scope:** Every commander command submission

---

### 2.3 Batch PTY Output Processing

**Location:** `src/pane.rs:154-178`, `pump()` function

**Current Code:**
```rust
pub(crate) fn pump(&mut self) -> bool {
    let mut any = false;
    // Read one message at a time
    match self.rx.try_recv() {
        Ok(bytes) => { ... }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => { ... }
    }
    any
}
```

**Issue:** Reads only one message per tick, causing multiple ticks to drain backlog.

**Recommendation:** Drain all available messages in a loop:
```rust
loop {
    match self.rx.try_recv() {
        Ok(bytes) => {
            any = true;
            self.parser.write(&bytes);
            self.view_dirty = true;
        }
        Err(TryRecvError::Empty) => break,
        Err(TryRecvError::Disconnected) => { ... }
    }
}
```

**Impact:** Medium - reduces latency when bursts of output arrive
**Risk:** Low - just processing available data instead of leaving it queued
**Effort:** 15 minutes
**Scope:** Every tick when PTY has output

---

## Priority 3: Higher Risk or Lower Impact

### 3.1 Optimize Layout Traversal

**Location:** `src/layout.rs:255-299`, `placement_at()`

**Current Pattern:** Recursive tree traversal on every placement lookup

**Recommendation:** 
- Add early exits when point is clearly outside area
- Pre-compute bounding boxes for subtrees
- Cache last-used placement path

**Impact:** Medium during mouse-heavy usage
**Risk:** Medium-High (core layout logic)
**Effort:** 2 hours
**Scope:** Every placement_at() call (render + mouse events)

---

### 3.2 Reduce Render Widget Allocations

**Location:** `src/main.rs:212-480`, render loop

**Current Pattern:** Creating new Style, Rect, Paragraph widgets every frame, even when unchanged.

**Recommendation:** 
- Cache static UI elements (settings button, frame borders when not focused)
- Only recompute styles when theme changes
- Reuse Rect calculations when size hasn't changed

**Impact:** Low-Medium (ratatui already does differential rendering)
**Risk:** Medium (requires careful cache invalidation)
**Effort:** 2 hours
**Scope:** Every frame

---

### 3.3 Optimize Pane Styled View Building

**Location:** `src/pane.rs:386-550`, `styled_view()` and `build_styled_view()`

**Current Code:** Already has caching with `view_dirty` flag (good!). However, still clones Text on every access:
```rust
if !self.view_dirty {
    if let Some(cached) = &self.cached_view {
        return cached.clone();
    }
}
```

**Recommendation:** Consider returning a reference instead of cloning, or use `Rc<Text>` for cheap cloning.

**Impact:** Low (Text is already optimized in ratatui)
**Risk:** Medium (lifetime management)
**Effort:** 1 hour
**Scope:** Every frame render for each pane

---

### 3.4 Lazy Layout Persistence

**Location:** `src/app.rs:395`, `persist_layout()`

**Current Code:** Saves layout to disk on every resize/pane operation

**Recommendation:** 
- Batch persistence operations
- Debounce saves (only save after N seconds of inactivity)
- Use background thread for file I/O

**Impact:** Low (file I/O is already fast for small files)
**Risk:** Low
**Effort:** 30 minutes
**Scope:** On every layout change

---

## Priority 4: Nice-to-Have Optimizations

### 4.1 Reduce Format! Macro Usage

**Locations:** Throughout `app.rs` (36 instances)

**Issue:** `format!()` always allocates a new String, even for simple cases.

**Recommendation:** Use `write!()` with reusable buffers or `concat!()` for compile-time strings.

**Impact:** Negligible
**Risk:** Minimal
**Effort:** 30 minutes
**Scope:** Various error messages and UI text

---

### 4.2 Optimize Agent Availability Checks

**Location:** `src/app.rs:2786`

**Current Code:**
```rust
.map(|pane| (pane.title.clone(), agent_index_for_command(&pane.command)))
```

**Recommendation:** Cache agent_index in Pane struct since command rarely changes.

**Impact:** Very Low
**Risk:** Low
**Effort:** 15 minutes
**Scope:** When opening panes/modals

---

### 4.3 Conditional Scrollbar Rendering

**Location:** `src/main.rs:419-445`

**Current Code:** Always computes scrollbar state even when not needed.

**Recommendation:** Only compute scrollbar when `needs_scrollbar()` returns true.

**Impact:** Very Low
**Risk:** Minimal
**Effort:** 10 minutes
**Scope:** Every frame render

---

## Critical Performance Observations

### What's Already Well-Optimized:

1. **View Caching:** `Pane` caches styled views with dirty flag ✓
2. **Dirty Rendering:** Only re-render when app state changes ✓
3. **Background Threads:** PTY I/O runs on separate threads ✓
4. **Event Polling:** 30ms poll with sleep between events ✓
5. **Incremental Parsing:** vt100 parser processes bytes incrementally ✓

### Potential Bottlenecks:

1. **Linear Search:** No spatial indexing for pane lookups (Priority 1.2)
2. **Layout Traversal:** Tree walk on every mouse event (Priority 3.1)
3. **Clone Overhead:** Unnecessary clones in hot paths (Priority 1.1)
4. **Message Queue:** Single-message processing per tick (Priority 2.3)

---

## Implementation Plan

### Phase 1: Quick Wins (1-2 hours total)
1. Remove drag_resize clone (5 min)
2. Drain PTY message queue (15 min)
3. Reduce commander string allocations (30 min)
4. Conditional scrollbar (10 min)
5. Format! cleanup (30 min)

### Phase 2: Core Improvements (3-4 hours total)
1. Pane lookup HashMap (30 min)
2. Placement caching (20 min)
3. Mouse event optimization (1 hour)
4. Agent index caching (15 min)
5. Layout persistence batching (30 min)

### Phase 3: Advanced (4-6 hours total)
1. Layout traversal optimizations (2 hours)
2. Render widget caching (2 hours)
3. Pane view reference semantics (1 hour)
4. Profiling-guided optimizations (remaining time)

---

## Profiling Recommendations

Before implementing Priority 3+ changes, profile the application:

```bash
# Linux with perf
cargo build --release
perf record -g ./target/release/split_tui
perf report

# Alternative: cargo-flamegraph
cargo install flamegraph
cargo flamegraph --root

# Or use coz for causal profiling
cargo install coz
```

Focus profiling on:
1. Mouse event handling time
2. Frame rendering time
3. Layout computation time

---

## Risk Mitigation

### Testing Strategy

1. **Before changes:** Document current behavior with manual tests:
   - Mouse-driven resize
   - Pane swapping
   - Modal interactions
   - Commander commands
   - Window resize

2. **After each change:** Run same test suite

3. **Automated tests:** Consider adding unit tests for:
   - `placement_at()` correctness
   - `handle_mouse()` routing
   - Pane lookup map consistency

### Rollback Plan

Each optimization should be:
1. In a separate commit
2. Easy to revert
3. Feature-flagged if risky (e.g., `if USE_PANE_MAP { ... } else { ... }`)

---

## Estimated Performance Gains

| Priority | Improvement | Risk | Effort | Expected Gain |
|----------|-------------|------|--------|---------------|
| 1.1 | Drag clone removal | Very Low | 2 min | 5-10% mouse handling |
| 1.2 | Pane lookup map | Low | 30 min | 15-25% render time |
| 1.3 | Placement caching | Low | 20 min | 10-15% modal render |
| 2.1 | Mouse routing | Medium | 1 hr | 10-20% mouse events |
| 2.2 | String allocs | Low | 30 min | 2-5% commander |
| 2.3 | Batch PTY | Low | 15 min | 10-30% output latency |
| 3.1 | Layout traversal | Medium | 2 hrs | 15-25% placement |
| 3.2 | Widget caching | Medium | 2 hrs | 5-10% render |
| 3.3 | View references | Medium | 1 hr | 2-5% render |

**Cumulative Expected Gain:** 20-40% overall responsiveness improvement

---

## Conclusion

The most impactful, lowest-risk improvements are:

1. **Remove unnecessary clones** (Priority 1.1, 1.2) - Quick wins with measurable impact
2. **Batch PTY processing** (Priority 2.3) - Reduces output latency
3. **Optimize mouse routing** (Priority 2.1) - Reduces event processing time

These three alone should provide 20-30% improvement in responsiveness with minimal risk.

Higher-priority layout optimizations (3.1) should wait until profiling confirms they're bottlenecks.
