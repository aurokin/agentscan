# Security

## What agentscan reads

Everything agentscan does runs locally on your machine.

- The CLI and daemon read tmux metadata (sessions, windows, panes, process
  info) to discover and classify agent panes.
- When provider identity is already established and richer signals are
  unavailable, agentscan may capture the visible screen contents of that pane
  as a provider-scoped status fallback (reported with
  `status.source="pane_output"`). Captured text is used only to classify
  busy/idle state; it is not logged, stored, or transmitted.

## What leaves your machine

Nothing. The CLI and daemon make no network calls and collect no telemetry.

The desktop app's only network call is an update check against the GitHub
releases API (`api.github.com`). It fails silently when offline and never
uploads anything.

## Reporting a vulnerability

Please report vulnerabilities privately via
[GitHub private vulnerability reporting](https://github.com/aurokin/agentscan/security/advisories/new)
or by email to auro@hsadler.com. Please do not open public issues for
security reports.
