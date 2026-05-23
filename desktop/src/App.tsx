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

type PickerRow = {
  key: string;
  pane_id: string;
  provider: string | null;
  status: { kind: string };
  display_label: string;
  location_tag: string;
};

type LoadState =
  | { status: "loading" }
  | {
      status: "ready";
      profiles: DesktopProfile[];
      preflight: AgentscanPreflight;
      picker: PickerState;
    }
  | { status: "failed"; message: string };

type PickerState =
  | { status: "loading" }
  | { status: "ready"; rows: PickerRow[] }
  | { status: "failed"; message: string };

function App() {
  const [state, setState] = useState<LoadState>({ status: "loading" });
  const [isRefreshing, setIsRefreshing] = useState(false);

  useEffect(() => {
    let cancelled = false;

    async function loadShellState() {
      try {
        const [profiles, preflight] = await Promise.all([
          invoke<DesktopProfile[]>("local_profiles"),
          invoke<AgentscanPreflight>("preflight_agentscan"),
        ]);

        if (!cancelled) {
          setState({
            status: "ready",
            profiles,
            preflight,
            picker: { status: "loading" },
          });
        }

        try {
          const pickerRows = await invoke<PickerRow[]>("load_picker_rows");

          if (!cancelled) {
            setState({
              status: "ready",
              profiles,
              preflight,
              picker: { status: "ready", rows: pickerRows },
            });
          }
        } catch (error) {
          if (!cancelled) {
            setState({
              status: "ready",
              profiles,
              preflight,
              picker: { status: "failed", message: errorMessage(error) },
            });
          }
        }
      } catch (error) {
        if (!cancelled) {
          setState({
            status: "failed",
            message: errorMessage(error),
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

  async function refreshPickerRows() {
    if (state.status !== "ready") {
      return;
    }

    setIsRefreshing(true);
    setState({ ...state, picker: { status: "loading" } });

    try {
      const rows = await invoke<PickerRow[]>("load_picker_rows");
      setState((current) =>
        current.status === "ready"
          ? { ...current, picker: { status: "ready", rows } }
          : current,
      );
    } catch (error) {
      setState((current) =>
        current.status === "ready"
          ? { ...current, picker: { status: "failed", message: errorMessage(error) } }
          : current,
      );
    } finally {
      setIsRefreshing(false);
    }
  }

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
        <>
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

          <section className="picker-panel" aria-label="Local picker rows">
            <div className="panel-heading">
              <h2>Picker</h2>
              <button type="button" onClick={refreshPickerRows} disabled={isRefreshing}>
                {isRefreshing ? "Refreshing" : "Refresh"}
              </button>
            </div>

            <PickerRows state={state.picker} />
          </section>
        </>
      ) : (
        <section className="panel" aria-live="polite">
          <h2>{state.status === "loading" ? "Loading" : "Unable to load"}</h2>
          <p>{state.status === "failed" ? state.message : "Waiting for backend response."}</p>
        </section>
      )}
    </main>
  );
}

function PickerRows({ state }: { state: PickerState }) {
  if (state.status === "loading") {
    return <p className="muted">Loading picker rows.</p>;
  }

  if (state.status === "failed") {
    return (
      <div className="error-state" role="alert">
        <h3>Unable to load picker rows</h3>
        <p>{state.message}</p>
      </div>
    );
  }

  if (state.rows.length === 0) {
    return <p className="muted">No picker rows are available.</p>;
  }

  return (
    <ul className="picker-list">
      {state.rows.map((row) => (
        <li key={`${row.key}-${row.pane_id}`}>
          <kbd>{row.key}</kbd>
          <div className="picker-row-main">
            <span>{row.display_label}</span>
            <small>{row.location_tag}</small>
          </div>
          <div className="picker-row-meta">
            <span>{row.provider ?? "unknown"}</span>
            <small>{row.status.kind}</small>
          </div>
        </li>
      ))}
    </ul>
  );
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export default App;
