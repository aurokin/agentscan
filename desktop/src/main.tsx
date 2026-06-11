import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import DockApp from "./DockApp";
import SettingsApp from "./SettingsApp";
import "./styles.css";

const root = document.getElementById("root");

if (!root) {
  throw new Error("missing root element");
}

// One Vite entry serves both windows; the window label routes to the right app.
// The "settings" window (declared hidden in tauri.conf.json) gets SettingsApp;
// ANYTHING else is the dock (label "main") — keep else-is-dock rather than
// tightening to label === "main", mirroring PrefsBridge.resolveMode, which the
// Effect services use to resolve the same rule per window.
const isSettings = getCurrentWebviewWindow().label === "settings";

createRoot(root).render(
  <StrictMode>{isSettings ? <SettingsApp /> : <DockApp />}</StrictMode>,
);
