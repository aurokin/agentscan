import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type DesktopProfile = {
  id: string;
  name: string;
  kind: "local";
};

type AgentscanPreflight = {
  binary: string;
  ok: boolean;
  version: string | null;
  error: string | null;
};

type LoadState =
  | { status: "loading" }
  | { status: "ready"; profiles: DesktopProfile[]; preflight: AgentscanPreflight }
  | { status: "failed"; message: string };

function App() {
  const [state, setState] = useState<LoadState>({ status: "loading" });

  useEffect(() => {
    let cancelled = false;

    async function loadShellState() {
      try {
        const [profiles, preflight] = await Promise.all([
          invoke<DesktopProfile[]>("local_profiles"),
          invoke<AgentscanPreflight>("preflight_agentscan"),
        ]);

        if (!cancelled) {
          setState({ status: "ready", profiles, preflight });
        }
      } catch (error) {
        if (!cancelled) {
          setState({
            status: "failed",
            message: error instanceof Error ? error.message : String(error),
          });
        }
      }
    }

    void loadShellState();

    return () => {
      cancelled = true;
    };
  }, []);

  const statusText = useMemo(() => {
    if (state.status === "loading") {
      return "Checking local CLI";
    }

    if (state.status === "failed") {
      return "IPC failed";
    }

    return state.preflight.ok ? "Local CLI ready" : "Local CLI unavailable";
  }, [state]);

  return (
    <main className="app-shell">
      <section className="summary">
        <div>
          <p className="eyebrow">agentscan desktop</p>
          <h1>Local agent workspace</h1>
        </div>
        <span className="status-pill">{statusText}</span>
      </section>

      {state.status === "ready" ? (
        <section className="content-grid" aria-label="Desktop shell state">
          <div className="panel">
            <h2>Profiles</h2>
            <ul className="profile-list">
              {state.profiles.map((profile) => (
                <li key={profile.id}>
                  <span>{profile.name}</span>
                  <small>{profile.kind}</small>
                </li>
              ))}
            </ul>
          </div>

          <div className="panel">
            <h2>Preflight</h2>
            <dl className="preflight">
              <div>
                <dt>Binary</dt>
                <dd>{state.preflight.binary}</dd>
              </div>
              <div>
                <dt>Version</dt>
                <dd>{state.preflight.version ?? "Unavailable"}</dd>
              </div>
              {!state.preflight.ok ? (
                <div>
                  <dt>Error</dt>
                  <dd>{state.preflight.error ?? "Unknown failure"}</dd>
                </div>
              ) : null}
            </dl>
          </div>
        </section>
      ) : (
        <section className="panel" aria-live="polite">
          <h2>{state.status === "loading" ? "Loading" : "Unable to load"}</h2>
          <p>{state.status === "failed" ? state.message : "Waiting for backend response."}</p>
        </section>
      )}
    </main>
  );
}

export default App;
