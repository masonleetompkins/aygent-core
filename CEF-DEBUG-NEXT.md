# CEF embed — stop guessing, read the source

## What the logs PROVE (cumulative, all builds)
- CEF inits OK, browser created + bound to tab, Google **loads** (`url loaded https://www.google.com/`).
- `window_info(set_as_child): parent=0x<nonnull> view=0x0 wl=0` — parent set, CEF's own view handle 0x0.
- Window titlebar shows "AYGENT Helper (Renderer)" — renderer subprocess.
- Page loads but **zero pixels** render into our pane. Tried: Below/Above z-order, transparent pane. None worked.

## The real hypothesis (not z-order / not transparency)
CEF ALLOY windowed rendering creates its OWN NSView child inside our wrapper, but:
1. That child is never resized to fill the wrapper (autoresizing off), OR
2. Our wrapper NSView frame is wrong (Y-flip / zero size) so the CEF child is off-screen, OR
3. `set_as_child` on macOS in cef-rs 151 needs a specific dance we're missing.

## READ THESE ON THE MAC (authoritative — do NOT guess):
1. cef-rs set_as_child impl:
   grep -rn "fn set_as_child" ~/.cargo/registry/src/*/cef-151.0.0+151.3.11/src/
   then read that function fully — what fields it sets on WindowInfo.

2. Any windowed/native example in cef-rs that embeds into a parent NSView:
   ls ~/Documents/aygent/cef-spike/vendor/cef-rs/examples/
   find ~/Documents/aygent/cef-spike/vendor/cef-rs -name "*.rs" | xargs grep -ln "set_as_child\|parent_view\|WindowInfo"
   Read how the WORKING example sizes/parents the browser view.

3. CEF's own doc on windowed mac embedding:
   The cefsimple mac (shared/mac/mod.rs) — how it creates the NSWindow + content
   view + sizes the browser. Our wrapper must match that sizing/autoresize.

## Likely fix directions (confirm against source first)
- After browser create, get the CEF browser's host window handle
  (browser.host().window_handle()) — that's the NSView CEF made. Set its
  autoresizingMask = width|height AND setFrame to fill the wrapper bounds.
- OR set the wrapper's autoresizesSubviews = YES + give the CEF child a proper frame.
- Verify the wrapper frame Y-flip in place_wrapper isn't putting it off-screen
  (log the wrapper's actual .frame after setFrame, compare to window bounds).
