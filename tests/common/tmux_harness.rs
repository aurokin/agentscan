struct TestHarness {
    _tempdir: TempDir,
    tmux_tmpdir: PathBuf,
    tmux_socket_path: PathBuf,
    agentscan_socket_path: PathBuf,
    cache_path: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    display_popup_launch_count: AtomicUsize,
}

const AGENTSCAN_TMUX_SOCKET_ENV_VAR: &str = "AGENTSCAN_TMUX_SOCKET";

impl TestHarness {
    fn new() -> Result<Self> {
        let tempdir = tempfile::tempdir().context("failed to create tempdir")?;
        fs::set_permissions(tempdir.path(), fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to chmod {}", tempdir.path().display()))?;
        let tmux_tmpdir = tempdir.path().join("tmux");
        fs::create_dir_all(&tmux_tmpdir)
            .with_context(|| format!("failed to create {}", tmux_tmpdir.display()))?;
        let tmux_socket_path = tmux_default_socket_path(&tmux_tmpdir)?;
        let tmux_socket_dir = tmux_socket_path
            .parent()
            .context("failed to derive tmux socket directory")?;
        fs::create_dir_all(tmux_socket_dir)
            .with_context(|| format!("failed to create {}", tmux_socket_dir.display()))?;
        fs::set_permissions(&tmux_tmpdir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to chmod {}", tmux_tmpdir.display()))?;
        fs::set_permissions(tmux_socket_dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to chmod {}", tmux_socket_dir.display()))?;

        Ok(Self {
            tmux_socket_path,
            agentscan_socket_path: tempdir.path().join("agentscan.sock"),
            cache_path: tempdir.path().join("cache.json"),
            stdout_path: tempdir.path().join("daemon.stdout.log"),
            stderr_path: tempdir.path().join("daemon.stderr.log"),
            display_popup_launch_count: AtomicUsize::new(0),
            tmux_tmpdir,
            _tempdir: tempdir,
        })
    }

    fn start_session(&self, session_name: &str, command: &str) -> Result<String> {
        let status = self
            .tmux_command()
            .arg("-f")
            .arg("/dev/null")
            .args(["new-session", "-d", "-s", session_name, command])
            .status()
            .with_context(|| format!("failed to start tmux session {session_name}"))?;
        if !status.success() {
            bail!("tmux new-session failed for {session_name} with status {status}");
        }

        let output = self.tmux_output([
            "display-message",
            "-p",
            "-t",
            &format!("{session_name}:0.0"),
            "#{pane_id}",
        ])?;
        Ok(output.trim().to_string())
    }

    fn split_window(&self, target: &str, command: &str) -> Result<String> {
        let output = self.tmux_output([
            "split-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            target,
            command,
        ])?;
        Ok(output.trim().to_string())
    }

    fn new_window(&self, session_name: &str, command: &str) -> Result<String> {
        let output = self.tmux_output([
            "new-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            session_name,
            command,
        ])?;
        Ok(output.trim().to_string())
    }

    fn seed_tui_two_pane_cache(&self, root_pane_id: &str, split_pane_id: &str) -> Result<()> {
        self.agentscan([
            "tmux",
            "set-metadata",
            "--pane-id",
            root_pane_id,
            "--provider",
            "codex",
            "--label",
            "Root Task",
            "--state",
            "idle",
        ])?;
        self.agentscan([
            "tmux",
            "set-metadata",
            "--pane-id",
            split_pane_id,
            "--provider",
            "claude",
            "--label",
            "Split Task",
            "--state",
            "busy",
        ])?;
        self.agentscan(["-f", "cache", "validate"])
    }

    fn start_daemon(&self) -> Result<DaemonHandle> {
        let stdout = fs::File::create(&self.stdout_path)
            .with_context(|| format!("failed to create {}", self.stdout_path.display()))?;
        let stderr = fs::File::create(&self.stderr_path)
            .with_context(|| format!("failed to create {}", self.stderr_path.display()))?;

        let child = Command::new(agentscan_bin()?)
            .args(["daemon", "run"])
            .env_remove("TMUX")
            .env("TMUX_TMPDIR", &self.tmux_tmpdir)
            .env(AGENTSCAN_TMUX_SOCKET_ENV_VAR, &self.tmux_socket_path)
            .env("AGENTSCAN_CACHE_PATH", &self.cache_path)
            .env("AGENTSCAN_SOCKET_PATH", &self.agentscan_socket_path)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .context("failed to start agentscan daemon")?;

        Ok(DaemonHandle {
            child,
            stdout_path: self.stdout_path.clone(),
            stderr_path: self.stderr_path.clone(),
        })
    }

    fn attach_client(&self, session_name: &str) -> Result<AttachedClientHandle> {
        let existing_ttys = self.client_ttys()?;
        let mut child = self.spawn_attached_client(session_name)?;

        let tty = self.wait_for_new_client_tty(&existing_ttys, &mut child)?;
        Ok(AttachedClientHandle { child, tty })
    }

    fn spawn_attached_client(&self, session_name: &str) -> Result<Child> {
        let mut command = Command::new("script");
        command.arg("-q");

        if cfg!(target_os = "macos") || cfg!(target_os = "freebsd") || cfg!(target_os = "openbsd") {
            command
                .arg("/dev/null")
                .arg("tmux")
                .arg("-S")
                .arg(&self.tmux_socket_path)
                .args(["attach-session", "-t", session_name]);
        } else {
            let attach_command = format!(
                "tmux -S {} attach-session -t {}",
                shell_escape_path(&self.tmux_socket_path),
                shell_escape(session_name),
            );
            command.args(["-c", &attach_command, "/dev/null"]);
        }

        if std::env::var_os("TERM").is_none() {
            command.env("TERM", "xterm-256color");
        }

        command
            .env_remove("TMUX")
            .env("TMUX_TMPDIR", &self.tmux_tmpdir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to attach tmux client to {session_name}"))
    }

    fn send_title_escape(&self, pane_id: &str, title: &str) -> Result<()> {
        self.tmux([
            "send-keys",
            "-t",
            pane_id,
            &format!("printf '\\033]2;{title}\\033\\\\'"),
            "Enter",
        ])
    }

    fn agentscan<I, S>(&self, args: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = self.agentscan_command()?;
        for arg in args {
            command.arg(arg.as_ref());
        }

        let output = command
            .output()
            .context("failed to execute agentscan command")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("agentscan command failed: {}", stderr.trim());
        }

        Ok(())
    }

    fn agentscan_output<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = self.agentscan_command()?;
        for arg in args {
            command.arg(arg.as_ref());
        }

        let output = command
            .output()
            .context("failed to execute agentscan command")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("agentscan command failed: {}", stderr.trim());
        }

        String::from_utf8(output.stdout).context("agentscan output was not valid UTF-8")
    }

    fn agentscan_output_with_tmux_tmpdir<I, S>(&self, args: I, tmux_tmpdir: &Path) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = self.agentscan_command()?;
        command.env("TMUX_TMPDIR", tmux_tmpdir);
        for arg in args {
            command.arg(arg.as_ref());
        }

        let output = command
            .output()
            .context("failed to execute agentscan command")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("agentscan command failed: {}", stderr.trim());
        }

        String::from_utf8(output.stdout).context("agentscan output was not valid UTF-8")
    }

    fn agentscan_command(&self) -> Result<Command> {
        let mut command = Command::new(agentscan_bin()?);
        command.env_remove("TMUX");
        command.env("TMUX_TMPDIR", &self.tmux_tmpdir);
        command.env(AGENTSCAN_TMUX_SOCKET_ENV_VAR, &self.tmux_socket_path);
        command.env("AGENTSCAN_CACHE_PATH", &self.cache_path);
        command.env("AGENTSCAN_SOCKET_PATH", &self.agentscan_socket_path);
        Ok(command)
    }

    fn tmux<I, S>(&self, args: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = self.tmux_command();
        for arg in args {
            command.arg(arg.as_ref());
        }

        let status = command.status().context("failed to execute tmux command")?;
        if !status.success() {
            bail!("tmux command failed with status {status}");
        }

        Ok(())
    }

    fn tmux_output<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = self.tmux_command();
        for arg in args {
            command.arg(arg.as_ref());
        }

        let output = command.output().context("failed to execute tmux command")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("tmux command failed: {}", stderr.trim());
        }

        String::from_utf8(output.stdout).context("tmux output was not valid UTF-8")
    }

    fn supports_display_popup_key_injection(&self) -> Result<bool> {
        if std::env::var_os("AGENTSCAN_RUN_DISPLAY_POPUP_TESTS").is_some() {
            return Ok(true);
        }

        Ok(self
            .tmux_version()?
            .as_deref()
            .and_then(parse_tmux_version)
            .is_some_and(|version| version >= (3, 6)))
    }

    fn tmux_version(&self) -> Result<Option<String>> {
        let output = self
            .tmux_command()
            .arg("-V")
            .output()
            .context("failed to execute tmux -V")?;
        if !output.status.success() {
            return Ok(None);
        }

        let stdout = String::from_utf8(output.stdout).context("tmux -V output was not UTF-8")?;
        Ok(stdout
            .trim()
            .strip_prefix("tmux ")
            .map(|version| version.to_string()))
    }

    fn start_agentscan_tui_pane(&self, target: &str, extra_args: &[&str]) -> Result<String> {
        let tui_command = self.agentscan_tui_command(extra_args)?;
        self.split_window(target, &tui_command)
    }

    fn start_agentscan_display_popup(
        &self,
        client_tty: &str,
        extra_args: &[&str],
    ) -> Result<DisplayPopupHandle> {
        let launch_index = self
            .display_popup_launch_count
            .fetch_add(1, Ordering::Relaxed);
        let token = display_popup_token(client_tty, launch_index);
        let ready_path = self._tempdir.path().join(format!("{token}.ready"));
        let done_path = self._tempdir.path().join(format!("{token}.done"));
        let stderr_path = self._tempdir.path().join(format!("{token}.stderr.log"));
        let tui_command =
            self.agentscan_display_popup_command(extra_args, &ready_path, &done_path)?;
        let stderr = fs::File::create(&stderr_path)
            .with_context(|| format!("failed to create {}", stderr_path.display()))?;

        let child = self
            .tmux_command()
            .args([
                "display-popup",
                "-E",
                "-c",
                client_tty,
                "-w",
                "90%",
                "-h",
                "24",
                &tui_command,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .context("failed to start tmux display-popup")?;

        Ok(DisplayPopupHandle {
            child,
            ready_path,
            done_path,
            stderr_path,
        })
    }

    fn agentscan_tui_command(&self, extra_args: &[&str]) -> Result<String> {
        let mut command = format!(
            "TMUX_TMPDIR={} AGENTSCAN_TMUX_SOCKET={} AGENTSCAN_CACHE_PATH={} AGENTSCAN_SOCKET_PATH={} {} tui",
            shell_escape_path(&self.tmux_tmpdir),
            shell_escape_path(&self.tmux_socket_path),
            shell_escape_path(&self.cache_path),
            shell_escape_path(&self.agentscan_socket_path),
            shell_escape_path(&agentscan_bin()?)
        );
        for arg in extra_args {
            command.push(' ');
            command.push_str(&shell_escape(arg));
        }
        Ok(command)
    }

    fn agentscan_display_popup_command(
        &self,
        extra_args: &[&str],
        ready_path: &Path,
        done_path: &Path,
    ) -> Result<String> {
        let mut command = format!(
            "TMUX_TMPDIR={} AGENTSCAN_TMUX_SOCKET={} AGENTSCAN_CACHE_PATH={} AGENTSCAN_SOCKET_PATH={} AGENTSCAN_TUI_READY_PATH={} AGENTSCAN_TUI_DONE_PATH={} {} tui",
            shell_escape_path(&self.tmux_tmpdir),
            shell_escape_path(&self.tmux_socket_path),
            shell_escape_path(&self.cache_path),
            shell_escape_path(&self.agentscan_socket_path),
            shell_escape_path(ready_path),
            shell_escape_path(done_path),
            shell_escape_path(&agentscan_bin()?)
        );
        for arg in extra_args {
            command.push(' ');
            command.push_str(&shell_escape(arg));
        }
        Ok(command)
    }

    fn send_keys_to_client<I, S>(&self, client_tty: &str, keys: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = self.tmux_command();
        command.args(["send-keys", "-K", "-c", client_tty]);
        for key in keys {
            command.arg(key.as_ref());
        }

        let status = command
            .status()
            .context("failed to send keys to tmux client")?;
        if !status.success() {
            bail!("tmux send-keys -K -c failed with status {status}");
        }

        Ok(())
    }

    fn capture_pane(&self, pane_id: &str) -> Result<String> {
        self.tmux_output(["capture-pane", "-p", "-t", pane_id])
    }

    fn pane_exists(&self, pane_id: &str) -> Result<bool> {
        Ok(self
            .tmux_output(["list-panes", "-a", "-F", "#{pane_id}"])?
            .lines()
            .any(|listed_pane_id| listed_pane_id.trim() == pane_id))
    }

    fn client_ttys(&self) -> Result<Vec<String>> {
        Ok(self
            .client_rows()?
            .into_iter()
            .map(|row| row.client_tty)
            .collect())
    }

    fn client_pane_id(&self, client_tty: &str) -> Result<Option<String>> {
        Ok(self
            .client_rows()?
            .into_iter()
            .find(|row| row.client_tty == client_tty)
            .map(|row| row.pane_id))
    }

    fn wait_for_new_client_tty(
        &self,
        existing_ttys: &[String],
        client: &mut Child,
    ) -> Result<String> {
        let deadline = Instant::now() + DAEMON_TIMEOUT;
        loop {
            if let Some(status) = client
                .try_wait()
                .context("failed to poll attached tmux client")?
            {
                bail!("attached tmux client exited before registering with status {status}");
            }

            for row in self.client_rows()? {
                if !existing_ttys.iter().any(|tty| tty == &row.client_tty) {
                    return Ok(row.client_tty);
                }
            }

            if Instant::now() >= deadline {
                bail!("timed out waiting for attached tmux client");
            }

            sleep(POLL_INTERVAL);
        }
    }

    fn wait_for_client_pane(&self, client: &mut AttachedClientHandle, pane_id: &str) -> Result<()> {
        let deadline = Instant::now() + DAEMON_TIMEOUT;
        loop {
            client.ensure_running()?;

            for row in self.client_rows()? {
                if row.client_tty == client.tty && row.pane_id == pane_id {
                    return Ok(());
                }
            }

            if Instant::now() >= deadline {
                bail!(
                    "timed out waiting for client {} to focus pane {pane_id}",
                    client.tty
                );
            }

            sleep(POLL_INTERVAL);
        }
    }

    fn wait_for_client_key_table(&self, client_tty: &str, expected_key_table: &str) -> Result<()> {
        let deadline = Instant::now() + DAEMON_TIMEOUT;
        loop {
            let output = self.tmux_output([
                "list-clients",
                "-F",
                &format!("#{{client_tty}}{TMUX_TEST_DELIM}#{{client_key_table}}"),
            ])?;
            for line in output.lines() {
                let fields = split_tmux_test_fields(line);
                let listed_client_tty = fields.first().copied().unwrap_or_default().trim();
                let key_table = fields.get(1).copied().unwrap_or_default().trim();
                if listed_client_tty == client_tty && key_table == expected_key_table {
                    return Ok(());
                }
            }

            if Instant::now() >= deadline {
                bail!(
                    "timed out waiting for client {client_tty} to reach key table {expected_key_table}"
                );
            }

            sleep(POLL_INTERVAL);
        }
    }

    fn wait_for_pane_closed(&self, pane_id: &str) -> Result<()> {
        let deadline = Instant::now() + DAEMON_TIMEOUT;
        loop {
            if !self.pane_exists(pane_id)? {
                return Ok(());
            }

            if Instant::now() >= deadline {
                bail!("timed out waiting for TUI pane {pane_id} to close");
            }

            sleep(POLL_INTERVAL);
        }
    }

    fn wait_for_pane_contents<F>(&self, pane_id: &str, predicate: F) -> Result<String>
    where
        F: Fn(&str) -> bool,
    {
        let deadline = Instant::now() + DAEMON_TIMEOUT;
        loop {
            let contents = self.capture_pane(pane_id)?;
            if predicate(&contents) {
                return Ok(contents);
            }

            if Instant::now() >= deadline {
                bail!("timed out waiting for TUI pane {pane_id} contents to update");
            }

            sleep(POLL_INTERVAL);
        }
    }

    fn wait_for_cache<F>(&self, daemon: &mut DaemonHandle, predicate: F) -> Result<Value>
    where
        F: Fn(&Value) -> bool,
    {
        let deadline = Instant::now() + DAEMON_TIMEOUT;
        loop {
            daemon.ensure_running()?;

            if let Ok(contents) = fs::read_to_string(&self.cache_path)
                && let Ok(cache) = serde_json::from_str::<Value>(&contents)
                && predicate(&cache)
            {
                return Ok(cache);
            }

            if Instant::now() >= deadline {
                bail!(
                    "timed out waiting for cache update at {}",
                    self.cache_path.display()
                );
            }

            sleep(POLL_INTERVAL);
        }
    }

    fn wait_for_cache_file<F>(&self, predicate: F) -> Result<Value>
    where
        F: Fn(&Value) -> bool,
    {
        let deadline = Instant::now() + DAEMON_TIMEOUT;
        loop {
            if let Ok(contents) = fs::read_to_string(&self.cache_path)
                && let Ok(cache) = serde_json::from_str::<Value>(&contents)
                && predicate(&cache)
            {
                return Ok(cache);
            }

            if Instant::now() >= deadline {
                bail!(
                    "timed out waiting for cache update at {}",
                    self.cache_path.display()
                );
            }

            sleep(POLL_INTERVAL);
        }
    }

    fn wait_for_pane<F>(
        &self,
        daemon: &mut DaemonHandle,
        pane_id: &str,
        predicate: F,
    ) -> Result<Value>
    where
        F: Fn(&Value) -> bool,
    {
        self.wait_for_cache(daemon, |cache| {
            pane_from_cache(cache, pane_id).is_some_and(&predicate)
        })
        .and_then(|cache| {
            pane_from_cache(&cache, pane_id)
                .cloned()
                .with_context(|| format!("pane {pane_id} not found in cache"))
        })
    }

    fn tmux_command(&self) -> Command {
        let mut command = Command::new("tmux");
        command.arg("-S").arg(&self.tmux_socket_path);
        command.env_remove("TMUX");
        command.env("TMUX_TMPDIR", &self.tmux_tmpdir);
        command
    }

    fn client_rows(&self) -> Result<Vec<TmuxClientRow>> {
        let output = self.tmux_output([
            "list-clients",
            "-F",
            &format!("#{{client_tty}}{TMUX_TEST_DELIM}#{{pane_id}}"),
        ])?;
        let mut rows = Vec::new();
        for line in output.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let fields = split_tmux_test_fields(line);
            let client_tty = fields.first().copied().unwrap_or_default().trim();
            let pane_id = fields.get(1).copied().unwrap_or_default().trim();
            if client_tty.is_empty() {
                continue;
            }
            rows.push(TmuxClientRow {
                client_tty: client_tty.to_string(),
                pane_id: pane_id.to_string(),
            });
        }
        Ok(rows)
    }
}

impl Drop for TestHarness {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .arg("-S")
            .arg(&self.tmux_socket_path)
            .arg("kill-server")
            .env_remove("TMUX")
            .env("TMUX_TMPDIR", &self.tmux_tmpdir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

struct DaemonHandle {
    child: Child,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl DaemonHandle {
    fn ensure_running(&mut self) -> Result<()> {
        if let Some(status) = self
            .child
            .try_wait()
            .context("failed to poll daemon child")?
        {
            bail!(self.exit_message(status));
        }
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .context("failed to poll daemon child")?
            {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                bail!("timed out waiting for daemon to exit");
            }
            sleep(POLL_INTERVAL);
        }
    }

    fn exit_message(&self, status: ExitStatus) -> String {
        let stdout = read_log(&self.stdout_path);
        let stderr = read_log(&self.stderr_path);
        format!(
            "agentscan daemon exited unexpectedly with status {status}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        )
    }
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

struct DisplayPopupHandle {
    child: Child,
    ready_path: PathBuf,
    done_path: PathBuf,
    stderr_path: PathBuf,
}

impl DisplayPopupHandle {
    fn wait_until_ready(&mut self) -> Result<()> {
        let deadline = Instant::now() + DAEMON_TIMEOUT;
        loop {
            self.ensure_running()?;
            if self.ready_path.exists() {
                return Ok(());
            }

            if Instant::now() >= deadline {
                bail!("timed out waiting for display-popup command to start");
            }

            sleep(POLL_INTERVAL);
        }
    }

    fn wait_for_exit(&mut self) -> Result<()> {
        let deadline = Instant::now() + DAEMON_TIMEOUT;
        let mut display_popup_status = None;
        loop {
            if display_popup_status.is_none() {
                display_popup_status = self
                    .child
                    .try_wait()
                    .context("failed to poll display-popup command")?;
            }

            if let Ok(status) = fs::read_to_string(&self.done_path) {
                let status = status.trim();
                let _ = self.child.wait();
                if status == "0" {
                    return Ok(());
                }
                bail!("display-popup command exited with status {status}");
            }

            if let Some(status) = display_popup_status {
                bail!(
                    "tmux display-popup exited with status {status} before TUI wrote done marker\nstderr:\n{}",
                    read_log(&self.stderr_path)
                );
            }

            if Instant::now() >= deadline {
                let status_context = display_popup_status
                    .map(|status| format!("; tmux display-popup status: {status}"))
                    .unwrap_or_default();
                bail!(
                    "timed out waiting for display-popup command to exit{status_context}\nstderr:\n{}",
                    read_log(&self.stderr_path)
                );
            }

            sleep(POLL_INTERVAL);
        }
    }

    fn ensure_running(&mut self) -> Result<()> {
        if let Some(status) = self
            .child
            .try_wait()
            .context("failed to poll display-popup command")?
        {
            bail!("display-popup command exited before TUI was ready with status {status}");
        }
        Ok(())
    }
}

impl Drop for DisplayPopupHandle {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct AttachedClientHandle {
    child: Child,
    tty: String,
}

impl AttachedClientHandle {
    fn ensure_running(&mut self) -> Result<()> {
        if let Some(status) = self
            .child
            .try_wait()
            .context("failed to poll attached tmux client")?
        {
            bail!("attached tmux client exited unexpectedly with status {status}");
        }
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }
}

impl Drop for AttachedClientHandle {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

struct TmuxClientRow {
    client_tty: String,
    pane_id: String,
}

const TMUX_TEST_DELIM: &str = r"\037";

fn pane_from_cache<'a>(cache: &'a Value, pane_id: &str) -> Option<&'a Value> {
    cache["panes"]
        .as_array()?
        .iter()
        .find(|pane| pane["pane_id"].as_str() == Some(pane_id))
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn parse_tmux_version(version: &str) -> Option<(u32, u32)> {
    for numeric in version.split(|character: char| !character.is_ascii_digit() && character != '.')
    {
        if let Some((major, minor)) = parse_tmux_numeric_version(numeric) {
            return Some((major, minor));
        }
    }

    None
}

fn parse_tmux_numeric_version(numeric: &str) -> Option<(u32, u32)> {
    let mut parts = numeric.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn display_popup_token(client_tty: &str, launch_index: usize) -> String {
    let sanitized = client_tty
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("display-popup-{sanitized}-{launch_index}")
}

fn agentscan_bin() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_agentscan") {
        return Ok(PathBuf::from(path));
    }
    let cargo_bin = PathBuf::from(env!("CARGO_BIN_EXE_agentscan"));
    if cargo_bin.is_file() {
        return Ok(cargo_bin);
    }

    let current_exe = std::env::current_exe().context("failed to resolve current test binary")?;
    let debug_dir = current_exe
        .parent()
        .and_then(Path::parent)
        .context("failed to derive target debug directory")?;
    let candidate = debug_dir.join(format!("agentscan{}", std::env::consts::EXE_SUFFIX));
    if candidate.is_file() {
        return Ok(candidate);
    }

    bail!(
        "failed to find agentscan binary via CARGO_BIN_EXE_agentscan or {}",
        candidate.display()
    )
}

fn tmux_default_socket_path(tmux_tmpdir: &Path) -> Result<PathBuf> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("failed to determine current uid for tmux socket path")?;
    if !output.status.success() {
        bail!("`id -u` failed with status {}", output.status);
    }

    let uid = String::from_utf8(output.stdout).context("`id -u` output was not valid UTF-8")?;
    Ok(tmux_tmpdir.join(format!("tmux-{}/default", uid.trim())))
}

fn shell_escape_path(path: &Path) -> String {
    shell_escape(path.to_string_lossy().as_ref())
}

fn shell_escape(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '%'))
    {
        return value.to_string();
    }

    format!("'{}'", value.replace('\'', r"'\''"))
}

fn split_tmux_test_fields(line: &str) -> Vec<&str> {
    let fields: Vec<_> = line.split('\x1f').collect();
    if fields.len() > 1 {
        return fields;
    }

    line.split(TMUX_TEST_DELIM).collect()
}
