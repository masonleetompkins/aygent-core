import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import "./theme.css";
import { initTheme } from "./lib/theme";

initTheme(); // apply saved light/dark + accent before first paint

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
