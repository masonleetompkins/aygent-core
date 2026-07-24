import ReactDOM from "react-dom/client";
import { App } from "./App";
import "./theme.css";
import { initTheme } from "./lib/theme";

initTheme(); // apply saved light/dark + accent before first paint

// NOTE: StrictMode intentionally removed. It double-invokes effects in dev,
// which double-registered the Tauri event listener in Chat and mangled the
// stream (doubled tokens, then dropped tokens once we deduped). Production
// never double-invokes, so StrictMode was purely a dev-only footgun for our
// event-channel streaming model. One listener per turn = correct streaming.
ReactDOM.createRoot(document.getElementById("root")!).render(<App />);
