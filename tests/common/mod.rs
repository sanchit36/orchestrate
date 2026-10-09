//! Black-box harness: a scratch git repo, a scratch home, and a `bin/` of fake
//! executables put first on `PATH`. Tests drive only the `orchestrate` binary.
#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub struct Scratch {
    _dir: tempfile::TempDir,
    pub repo: PathBuf,
    pub home: PathBuf,
    pub bin: PathBuf,
}

impl Scratch {
    /// A git repo with one commit, an empty home and an empty fake `bin/`.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (repo, home, bin) = (root.join("repo"), root.join("home"), root.join("bin"));
        for d in [&repo, &home, &bin] {
            fs::create_dir_all(d).unwrap();
        }
        let s = Scratch {
            _dir: dir,
            repo,
            home,
            bin,
        };
        // An identity for commits made by orchestrate and by fake sessions.
        write(
            &s.home.join(".gitconfig"),
            "[user]\n\tname = t\n\temail = t@t\n[commit]\n\tgpgsign = false\n",
        );
        s.git(&["init", "-q", "-b", "main"]);
        s.git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        s
    }

    pub fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "git {args:?}: {err}");
        String::from_utf8(out.stdout).unwrap()
    }

    /// Write a file under the repo, creating parent directories.
    pub fn write(&self, rel: &str, content: &str) {
        write(&self.repo.join(rel), content);
    }

    /// `.orchestrate/config.toml` at the repo top level.
    pub fn repo_config(&self, content: &str) {
        self.write(".orchestrate/config.toml", content);
    }

    /// `~/.config/orchestrate/config.toml` in the scratch home.
    pub fn machine_config(&self, content: &str) {
        write(&self.home.join(".config/orchestrate/config.toml"), content);
    }

    /// Repo config for a local markdown tracker rooted at `specs/`.
    pub fn local_markdown_tracker(&self) {
        self.repo_config(
            "version = 1\n\n[tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\n",
        );
    }

    /// Fixture spec `specs/<spec>/spec.md` for the local markdown tracker.
    pub fn spec(&self, spec: &str, title: &str) {
        let body = format!("# {title}\n\nThe spec body.\n");
        self.write(&format!("specs/{spec}/spec.md"), &body);
    }

    /// Fixture ticket `specs/<spec>/<id>.md` with a `Status:` and a `Blocked by:` line.
    pub fn ticket(&self, spec: &str, id: &str, title: &str, status: &str, blocked_by: &[&str]) {
        let mut body = format!("# {title}\n\nStatus: {status}\n");
        if !blocked_by.is_empty() {
            body.push_str(&format!("Blocked by: {}\n", blocked_by.join(", ")));
        }
        body.push_str("\n## Acceptance criteria\n\n- [ ] it works\n");
        self.write(&format!("specs/{spec}/{id}.md"), &body);
    }

    /// Repo config for a local markdown tracker plus one `[[gates]]` entry running `gate`.
    pub fn local_markdown_tracker_with_gate(&self, gate: &str) {
        self.repo_config(&format!(
            "version = 1\n\n[tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\n\n\
             [[gates]]\nname = \"check\"\nrun = \"{gate}\"\n"
        ));
    }

    /// Commit everything in the repo, so the tracker files are on `HEAD`.
    pub fn commit_all(&self) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", "fixtures"]);
    }

    /// A fake `claude` that lands every ticket. It saves its prompt to
    /// `bin/prompt.<worktree-name>`, prints `session output`, commits
    /// `<worktree-name>.txt` and writes a `done` report with every criterion met.
    pub fn claude_lands(&self) {
        let script = format!(
            r#"slug=$(basename "$PWD")
cat > '{bin}/prompt.'"$slug"
echo "session output"
echo "$slug" > "$slug.txt"
git add "$slug.txt" && git commit -q -m "work $slug"
printf '{{"status":"done","acceptance_criteria":[{{"text":"it works","met":true,"evidence":"ok"}}],"notes":""}}' > .orchestrate-result.json"#,
            bin = self.bin.display()
        );
        self.fake("claude", &script);
    }

    /// The prompt the fake `claude` received in worktree `name`.
    pub fn prompt(&self, name: &str) -> String {
        fs::read_to_string(self.bin.join(format!("prompt.{name}"))).unwrap()
    }

    /// The default state root, `~/.local/state/orchestrate`.
    pub fn state_root(&self) -> PathBuf {
        self.home.join(".local/state/orchestrate")
    }

    /// The one per-repo directory under the state `root`.
    pub fn repo_state(&self, root: &Path) -> PathBuf {
        let dirs: Vec<_> = fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(dirs.len(), 1, "{dirs:?}");
        dirs.into_iter().next().unwrap()
    }

    /// The id of the one run under the default state root.
    pub fn run_id(&self) -> String {
        let runs = self.repo_state(&self.state_root()).join("runs");
        let ids: Vec<_> = fs::read_dir(runs)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(ids.len(), 1, "{ids:?}");
        ids[0].to_string_lossy().into_owned()
    }

    /// A fake executable first on `PATH`; each call's args are appended to `bin/<name>.calls`.
    pub fn fake(&self, name: &str, script: &str) {
        let path = self.bin.join(name);
        let calls = self.bin.join(format!("{name}.calls"));
        let content = format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\n{script}\n",
            calls.display()
        );
        write(&path, &content);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The args of each call a fake received, one line per call.
    pub fn calls(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.bin.join(format!("{name}.calls")))
            .map(|s| s.lines().map(String::from).collect())
            .unwrap_or_default()
    }

    /// Run `orchestrate` from the repo root.
    pub fn run(&self, args: &[&str]) -> Run {
        self.run_in(&self.repo, args)
    }

    /// Run `orchestrate` from `cwd` with the scratch home, fakes first on `PATH`, and no stdin.
    pub fn run_in(&self, cwd: &Path, args: &[&str]) -> Run {
        let path = format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap());
        let out = Command::new(env!("CARGO_BIN_EXE_orchestrate"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", &self.home)
            .env("PATH", path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_STATE_HOME")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        Run(out)
    }
}

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

pub struct Run(pub Output);

impl Run {
    pub fn code(&self) -> i32 {
        self.0.status.code().unwrap_or(-1)
    }

    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.0.stdout).into_owned()
    }

    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.0.stderr).into_owned()
    }

    /// Assert exit 0 and return stdout.
    pub fn ok(&self) -> String {
        let (out, err) = (self.stdout(), self.stderr());
        assert_eq!(self.code(), 0, "stdout:\n{out}\nstderr:\n{err}");
        out
    }

    /// Assert a non-zero exit and return stderr.
    pub fn fails(&self) -> String {
        assert_ne!(
            self.code(),
            0,
            "expected failure, stdout:\n{}",
            self.stdout()
        );
        self.stderr()
    }
}
