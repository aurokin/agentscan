import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";

export async function ensureNotificationPermission(): Promise<boolean> {
  try {
    if (await isPermissionGranted()) {
      return true;
    }
    return (await requestPermission()) === "granted";
  } catch {
    // Browser/jsdom and other non-Tauri hosts have no native notification bridge.
    return false;
  }
}

export async function notifyAgentFinished(
  provider: string | null,
  label: string,
): Promise<void> {
  try {
    if (!(await ensureNotificationPermission())) {
      return;
    }
    const providerLabel = notificationProviderLabel(provider);
    sendNotification({ title: `${providerLabel} finished`, body: label });
  } catch {
    // Native notification failures must never disturb the live dock subscription.
  }
}

export async function notifyAgentWaiting(
  provider: string | null,
  label: string,
): Promise<void> {
  try {
    if (!(await ensureNotificationPermission())) {
      return;
    }
    const providerLabel = notificationProviderLabel(provider);
    sendNotification({ title: `${providerLabel} waiting on you`, body: label });
  } catch {
    // Native notification failures must never disturb the live dock subscription.
  }
}

function notificationProviderLabel(provider: string | null): string {
  // Provider ids are the backend's serde names (snake_case, e.g. "cursor_cli");
  // soften the underscores for the human-facing title without inventing a new label.
  return provider === null ? "Agent" : provider.replace(/_/g, " ");
}
