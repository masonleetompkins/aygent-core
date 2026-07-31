# CEF-in-Tauri: VERDICT (2026-07-30, evidence-backed)

## The whole CEF Phase-1 climb WORKED up to the last mile:
CEF compiles, bundles (framework + 5 helpers + signed), launches, **inits OK**,
creates a browser, parents it, **LOADS google.com** — but renders ZERO pixels
into our pane. We proved (via 3 cef-rs source files) exactly WHY.

## GROUND TRUTH — why windowed embed can't work with this stack (3 confirmations):
1. **cefsimple/src/shared/simple_app.rs**: on macOS cefsimple NEVER calls
   `set_as_child`. Default = CEF **Views** (`browser_view_create` +
   `window_create_top_level` + `window.add_child_view`) = a CEF-OWNED top-level
   NSWindow. The `set_as_child` branch is `#[cfg(windows)]` only.
2. **cefsimple/src/mac/mod.rs**: CEF on macOS REQUIRES the host `NSApplication`
   to be a subclass conforming to `CefAppProtocol` (custom `sendEvent:` +
   `isHandlingSendEvent` + `CrAppControlProtocol`). aygent uses **tao's**
   NSApplication which does NOT conform. Without it, CEF's native view doesn't
   composite/handle events -> loads but no pixels. (This is the `CefAppProtocol`
   assert we saw in the very first crash trace.)
3. **cefsimple/src/shared/simple_handler/mac.rs** `window_from_browser()`:
   `browser.host().window_handle().cast::<NSView>()` -> `view.window()`. CEF's
   own view handle ALREADY has its OWN NSWindow. It never truly reparents into a
   foreign (tao) NSView even when set_as_child is passed a parent_view.

## CONCLUSION
**Windowed CEF-embedded-into-a-Tauri-owned-NSView is NOT supported by
cef-rs 151 + tao/Tauri.** `set_as_child` on macOS parents into CEF's own window,
not ours. Making tao's NSApplication CefAppProtocol-conformant is deep, brittle
swizzling on a class we don't own (fights every Tauri upgrade). Not worth it.

## THE CORRECT PATH: OSR (off-screen rendering)
`windowless_rendering_enabled: 1`. CEF renders to a pixel buffer via a
RenderHandler (`on_paint` gives us the BGRA buffer + dirty rects;
`get_view_rect` we supply the size). NO NSView reparent, NO NSApplication
conformance. We blit frames into a CALayer/bitmap in our pane and forward input
as CEF mouse/key events (browser.host().send_mouse_click_event etc). cef-rs
ships an **`osr` example** (~/Documents/aygent/cef-spike/vendor/cef-rs/examples/osr/).

### OSR build plan (next session):
1. Read osr example: `on_paint`, `get_view_rect`, RenderHandler wrap macro,
   send_mouse/key event APIs, windowless_rendering_enabled setting.
2. Settings/WindowInfo: windowless_rendering_enabled=1 (per-browser WindowInfo),
   external_message_pump stays (already wired + working).
3. RenderHandler.on_paint -> copy BGRA buffer -> draw into a CALayer contents
   (or an NSView we own that just displays the bitmap) sized to the pane.
   No CefAppProtocol needed because there's no CEF native view.
4. get_view_rect returns the pane size (device px, DPR-aware).
5. Input: capture clicks/keys/scroll on our display layer -> map to page coords
   -> browser.host().send_mouse_move/click/wheel + send_key_event. (We already
   have CDP as an alternative, but native CEF input is cleaner for OSR.)
6. Keep engine-cef feature + WKWebView kill-switch throughout.

## WHAT'S SHIPPED + WORKING RIGHT NOW (don't lose this):
The ENTIRE WKWebView browser works: multi-tab, address bar, back/fwd/refresh,
agent hand-off (checklist, Allow/Deny/Take, trusted CDP input, first-result),
persistent history, downloads (page-bridge reqwest -> agent folder), pulse.
Only WKWebView RENDERING (some sites wrong) + the download-via-CEF were the
reasons to migrate. CEF gets us correct rendering + native downloads via OSR.

## KILL-SWITCH TO SHIP TONIGHT
Build WITHOUT `--features engine-cef` = the working WKWebView browser, full
feature set. That's the ship-tonight path. CEF stays on the branch/feature for
the OSR build.

## CEF commit trail (all on origin/main, chronological):
199da8b(spike)->...->25f4171(init before Builder)->605702c(macOS paths)->
c154025(loop flag)->824d881(external message pump - MADE BROWSER CREATE WORK)->
2d7fd81(parent_view)->14f71c9(diag)->f73c76d(main-thread wrapper)->
e6b0b17(single set_as_child)->308595b(transparent pane)->d2199af(front above).
HEAD ~ d2199af. All the plumbing is RIGHT; the approach (windowed embed) is the
wrong one for this stack -> OSR.
