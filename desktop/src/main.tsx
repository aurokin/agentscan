import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import App from "./App";
import "./styles.css";

const root = document.getElementById("root");

if (!root) {
  throw new Error("missing root element");
}

// One Vite entry serves both windows; the window label decides which UI to render.
// The "settings" window (declared hidden in tauri.conf.json) shows the settings
// panel; everything else is the dock.
const mode = getCurrentWebviewWindow().label === "settings" ? "settings" : "dock";

createRoot(root).render(
  <StrictMode>
    <App mode={mode} />
  </StrictMode>,
);
