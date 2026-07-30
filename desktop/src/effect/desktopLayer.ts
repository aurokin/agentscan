import { Layer } from "effect";
import { layer as activationLayer } from "./Activation";
import { layer as appearanceLayer } from "./Appearance";
import { layer as debugLogLayer } from "./DebugLog";
import { layer as hostIpcLayer } from "./HostIpc";
import { layer as hostnameEnrichmentLayer } from "./HostnameEnrichment";
import { layer as liveConnectionLayer } from "./LiveConnection";
import { layer as notificationsLayer } from "./Notifications";
import { layer as preflightLayer } from "./Preflight";
import { layer as profilesLayer } from "./Profiles";
import { layer as summonHotkeyLayer } from "./SummonHotkey";

// One service graph is built per webview realm. Shared dependency layer values
// are deliberately reused here so Effect's layer memoization gives every
// consumer in that window the same service instance.
export const desktopLayer = Layer.mergeAll(
  liveConnectionLayer,
  profilesLayer,
  preflightLayer,
  appearanceLayer,
  notificationsLayer,
  summonHotkeyLayer,
  activationLayer,
  hostnameEnrichmentLayer,
  debugLogLayer,
  hostIpcLayer,
);
