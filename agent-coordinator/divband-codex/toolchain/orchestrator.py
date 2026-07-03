from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import shutil
import subprocess
import sys
import textwrap
import time
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Iterable


DOC_NAMES = [
    "README.md",
    "FEATURES.md",
    "IMPLEMENTATION.md",
    "REBASE_PLAYBOOK.md",
    "TESTING.md",
    "diffstat.txt",
    "file-inventory.txt",
]

FOCUSED_TESTS = [
    ("fmt", ["just", "fmt"], "."),
    (
        "test-codex-cli-best-profile",
        ["just", "test", "-p", "codex-cli", "best_profile"],
        ".",
    ),
    (
        "test-codex-cli-resume-best",
        ["just", "test", "-p", "codex-cli", "resume_best"],
        ".",
    ),
    (
        "test-app-server-profile-refresh",
        ["just", "test", "-p", "codex-app-server", "thread_profile_refresh"],
        ".",
    ),
    (
        "test-core-profile-failover",
        [
            "just",
            "test",
            "-p",
            "codex-core",
            "usage_limit_switches_profile_and_retries_turn",
        ],
        ".",
    ),
    (
        "test-mcp-cursor-session",
        ["just", "test", "-p", "codex-mcp-server", "cursor_session"],
        ".",
    ),
]

EXPANDED_TESTS = FOCUSED_TESTS + [
    ("test-codex-cli-package", ["just", "test", "-p", "codex-cli"], "."),
    ("test-codex-core-package", ["just", "test", "-p", "codex-core"], "."),
    (
        "test-app-server-package",
        ["just", "test", "-p", "codex-app-server"],
        ".",
    ),
    (
        "test-mcp-server-package",
        ["just", "test", "-p", "codex-mcp-server"],
        ".",
    ),
]

BUILD_COMMANDS = [
    (
        "build-divband-binaries",
        [
            "cargo",
            "build",
            "-p",
            "codex-cli",
            "-p",
            "codex-app-server",
            "-p",
            "codex-mcp-server",
        ],
        "codex-rs",
    )
]

BUILD_BINARIES = [
    "codex",
    "codex-profiles",
    "codex-app-server",
    "codex-mcp-server",
]


@dataclass
class CommandRecord:
    name: str
    command: list[str]
    cwd: str
    status: str
    returncode: int | None
    duration_seconds: float
    stdout: str | None = None
    stderr: str | None = None
    reason: str | None = None

    def as_json(self) -> dict[str, object]:
        return {
            "name": self.name,
            "command": self.command,
            "cwd": self.cwd,
            "status": self.status,
            "returncode": self.returncode,
            "durationSeconds": round(self.duration_seconds, 3),
            "stdout": self.stdout,
            "stderr": self.stderr,
            "reason": self.reason,
        }


class MigrationError(RuntimeError):
    pass


class MigrationRunner:
    def __init__(self, package_dir: Path, args: argparse.Namespace) -> None:
        self.package_dir = package_dir
        self.agent_coordinator_dir = package_dir.parent
        self.args = args
        self.source = args.source.resolve()
        self.output = args.output.resolve()
        self.patches_dir = args.patches.resolve()
        self.patch_file = args.patch_file.resolve()
        self.target_dir = None if args.no_shared_target_dir else args.target_dir.resolve()
        self.records: list[CommandRecord] = []
        self.manifest_dir = self.output / ".divband-migration"
        self.logs_dir = self.manifest_dir / "logs"
        self.prompts_dir = self.manifest_dir / "prompts"
        self.command_index = 0
        self.started_at = datetime.utcnow().isoformat(timespec="seconds") + "Z"

    def run(self) -> int:
        try:
            self.validate_inputs()
            if self.args.dry_run:
                self.print_plan()
                return 0
            if self.args.skip_apply:
                self.prepare_existing_output_repo()
            else:
                self.prepare_output_repo()
                self.write_source_docs()
                self.apply_overlay()
            self.write_migration_manifest(status="overlay-applied")
            self.run_agent_reviews()
            self.run_build_and_tests()
            self.write_migration_manifest(status="complete")
            self.write_report(status="complete")
            print(f"Divband Codex output is ready at {self.output}")
            return 0
        except KeyboardInterrupt:
            if not self.args.dry_run and self.output.exists():
                self.write_migration_manifest(status="interrupted", error="interrupted by user")
                self.write_report(status="interrupted", error="interrupted by user")
            print("migration interrupted by user", file=sys.stderr)
            return 130
        except Exception as exc:
            if not self.args.dry_run and self.output.exists():
                self.write_migration_manifest(status="failed", error=str(exc))
                self.write_report(status="failed", error=str(exc))
            print(f"migration failed: {exc}", file=sys.stderr)
            return 1

    def validate_inputs(self) -> None:
        if not self.source.is_dir():
            raise MigrationError(f"source clone does not exist: {self.source}")
        if not (self.source / ".git").exists():
            raise MigrationError(f"source path is not a git checkout: {self.source}")
        if self.args.patch_mode == "am":
            patch_files = self.patch_files()
            if not patch_files:
                raise MigrationError(f"no patch files found in {self.patches_dir}")
        elif not self.patch_file.is_file():
            raise MigrationError(f"patch file does not exist: {self.patch_file}")
        for doc_name in DOC_NAMES:
            path = self.package_dir / doc_name
            if not path.exists():
                raise MigrationError(f"required migration document is missing: {path}")
        if self.output == self.source:
            raise MigrationError("output path must not equal source path")
        if self.output == self.package_dir or self.output in self.package_dir.parents:
            raise MigrationError("output path must not be the migration package directory")

    def print_plan(self) -> None:
        plan = {
            "source": str(self.source),
            "output": str(self.output),
            "patchMode": self.args.patch_mode,
            "patches": [str(path) for path in self.patch_files()]
            if self.args.patch_mode == "am"
            else [str(self.patch_file)],
            "testProfile": self.args.test_profile,
            "build": not self.args.skip_build,
            "agents": self.args.agents,
            "skipApply": self.args.skip_apply,
            "targetDir": str(self.target_dir) if self.target_dir else "output/codex-rs/target",
        }
        print(json.dumps(plan, indent=2))

    def prepare_output_repo(self) -> None:
        if self.output.exists():
            if not self.args.force and not self.args.reuse_output:
                raise MigrationError(
                    f"output already exists: {self.output}; use --force or --reuse-output"
                )
            if self.args.force:
                safe_remove_tree(self.output)
        if not self.output.exists():
            print(f"copying vanilla Codex clone from {self.source} to {self.output}")
            shutil.copytree(self.source, self.output, symlinks=True)

        self.manifest_dir.mkdir(parents=True, exist_ok=True)
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        self.prompts_dir.mkdir(parents=True, exist_ok=True)

        self.run_command(
            "git-status-before-overlay",
            ["git", "status", "--short", "--branch"],
            cwd=self.output,
            check=True,
        )
        self.run_command(
            "git-checkout-output-branch",
            ["git", "checkout", "-B", self.args.output_branch],
            cwd=self.output,
            check=True,
        )

    def write_source_docs(self) -> None:
        docs_dir = self.manifest_dir / "source-docs"
        docs_dir.mkdir(parents=True, exist_ok=True)
        for doc_name in DOC_NAMES:
            shutil.copy2(self.package_dir / doc_name, docs_dir / doc_name)

    def apply_overlay(self) -> None:
        if self.args.patch_mode == "am":
            command = ["git", "am", "--3way", *[str(path) for path in self.patch_files()]]
            self.run_command("apply-overlay-git-am", command, cwd=self.output, check=True)
        else:
            self.run_command(
                "apply-overlay-git-apply",
                ["git", "apply", "--3way", str(self.patch_file)],
                cwd=self.output,
                check=True,
            )
            self.run_command(
                "stage-applied-overlay",
                ["git", "add", "-A"],
                cwd=self.output,
                check=True,
            )
            self.run_command(
                "commit-applied-overlay",
                ["git", "commit", "-m", "feat: apply divband codex overlay"],
                cwd=self.output,
                check=True,
            )
        self.run_command(
            "git-status-after-overlay",
            ["git", "status", "--short", "--branch"],
            cwd=self.output,
            check=True,
        )

    def prepare_existing_output_repo(self) -> None:
        if not self.output.is_dir():
            raise MigrationError(f"--skip-apply requires an existing output repo: {self.output}")
        self.manifest_dir.mkdir(parents=True, exist_ok=True)
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        self.prompts_dir.mkdir(parents=True, exist_ok=True)
        self.write_source_docs()
        self.run_command(
            "git-status-existing-output",
            ["git", "status", "--short", "--branch"],
            cwd=self.output,
            check=True,
        )

    def run_agent_reviews(self) -> None:
        if self.args.agents == "off":
            self.record_skip("cursor-review", "agent mode is off")
            self.record_skip("codex-review", "agent mode is off")
            return

        prompt = self.build_review_prompt()
        cursor_prompt = self.prompts_dir / "cursor-review.md"
        codex_prompt = self.prompts_dir / "codex-review.md"
        cursor_prompt.write_text(prompt, encoding="utf-8")
        codex_prompt.write_text(prompt, encoding="utf-8")

        self.run_cursor_review(prompt)
        self.run_codex_review(prompt)

    def run_cursor_review(self, prompt: str) -> None:
        command_text = os.environ.get("CURSOR_SESSION_AGENT_COMMAND", "cursor-agent")
        command_parts = shlex.split(command_text)
        if not command_parts:
            self.agent_unavailable("cursor-review", "CURSOR_SESSION_AGENT_COMMAND is empty")
            return
        program = shutil.which(command_parts[0])
        if program is None:
            self.agent_unavailable("cursor-review", f"{command_parts[0]} was not found")
            return

        cursor_home = Path(os.environ.get("CURSOR_SESSION_HOME", "/cursor-home"))
        auth_file = cursor_home / ".config" / "cursor" / "auth.json"
        if not auth_file.is_file():
            self.agent_unavailable("cursor-review", f"missing Cursor auth file: {auth_file}")
            return

        mode = os.environ.get("CURSOR_SESSION_MODE", "ask")
        model = os.environ.get("CURSOR_SESSION_MODEL", "auto")
        command = [
            program,
            *command_parts[1:],
            "-p",
            "--trust",
            "--mode",
            mode,
            "--model",
            model,
            "--output-format",
            "text",
            prompt,
        ]
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(cursor_home),
                "XDG_CONFIG_HOME": str(cursor_home / ".config"),
                "XDG_CACHE_HOME": str(cursor_home / ".cache"),
                "NPM_CONFIG_CACHE": str(cursor_home / ".npm"),
            }
        )
        self.run_command(
            "cursor-review",
            command,
            cwd=self.output,
            env=env,
            check=self.args.agents == "required",
            timeout=self.args.agent_timeout_seconds,
        )

    def run_codex_review(self, prompt: str) -> None:
        command_text = os.environ.get(
            "DIVBAND_CODEX_REVIEW_COMMAND",
            "codex exec --sandbox read-only",
        )
        command_parts = shlex.split(command_text)
        if not command_parts:
            self.agent_unavailable("codex-review", "DIVBAND_CODEX_REVIEW_COMMAND is empty")
            return
        program = shutil.which(command_parts[0])
        if program is None:
            self.agent_unavailable("codex-review", f"{command_parts[0]} was not found")
            return
        command = [program, *command_parts[1:], prompt]
        self.run_command(
            "codex-review",
            command,
            cwd=self.output,
            check=self.args.agents == "required",
            timeout=self.args.agent_timeout_seconds,
        )

    def agent_unavailable(self, name: str, reason: str) -> None:
        if self.args.agents == "required":
            raise MigrationError(f"{name} is required but unavailable: {reason}")
        self.record_skip(name, reason)

    def run_build_and_tests(self) -> None:
        env = os.environ.copy()
        env["CARGO_BUILD_JOBS"] = str(self.args.jobs)
        cargo_bin = str(Path.home() / ".cargo" / "bin")
        local_bin = str(Path.home() / ".local" / "bin")
        env["PATH"] = os.pathsep.join([cargo_bin, local_bin, env.get("PATH", "")])
        if self.target_dir is not None:
            self.target_dir.mkdir(parents=True, exist_ok=True)
            env["CARGO_TARGET_DIR"] = str(self.target_dir)

        if self.args.install_tools:
            self.ensure_tool("just", ["cargo", "install", "just"], env=env)
            self.ensure_tool(
                "cargo-nextest",
                ["cargo", "install", "--locked", "cargo-nextest"],
                env=env,
                binary_name="cargo-nextest",
            )

        if not self.args.skip_build:
            for name, command, cwd in BUILD_COMMANDS:
                self.run_command(
                    name,
                    command,
                    cwd=self.output / cwd,
                    env=env,
                    check=True,
                    timeout=self.args.command_timeout_seconds,
                )
            self.copy_build_artifacts()
        else:
            self.record_skip("build-divband-binaries", "--skip-build was set")

        if self.args.test_profile == "none":
            self.record_skip("tests", "test profile is none")
            return

        tests = FOCUSED_TESTS if self.args.test_profile == "focused" else EXPANDED_TESTS
        for name, command, cwd in tests:
            self.run_command(
                name,
                command,
                cwd=self.output / cwd,
                env=env,
                check=True,
                timeout=self.args.command_timeout_seconds,
            )

    def copy_build_artifacts(self) -> None:
        bin_dir = self.manifest_dir / "bin"
        bin_dir.mkdir(parents=True, exist_ok=True)
        target_debug = (self.target_dir or (self.output / "codex-rs" / "target")) / "debug"
        suffix = ".exe" if os.name == "nt" else ""
        copied = []
        for binary in BUILD_BINARIES:
            source = target_debug / f"{binary}{suffix}"
            if source.is_file():
                destination = bin_dir / source.name
                shutil.copy2(source, destination)
                copied.append(destination.name)
        if copied:
            self.record_skip(
                "copy-build-artifacts",
                "copied binaries into .divband-migration/bin: " + ", ".join(copied),
            )
        else:
            self.record_skip(
                "copy-build-artifacts",
                f"no expected binaries found in {target_debug}",
            )

        if self.args.run_fix:
            self.run_command(
                "fix-codex-core",
                ["just", "fix", "-p", "codex-core"],
                cwd=self.output,
                env=env,
                check=True,
                timeout=self.args.command_timeout_seconds,
            )

    def ensure_tool(
        self,
        label: str,
        install_command: list[str],
        env: dict[str, str],
        binary_name: str | None = None,
    ) -> None:
        binary = binary_name or label
        if shutil.which(binary, path=env.get("PATH")) is not None:
            self.record_skip(f"install-{label}", f"{binary} is already available")
            return
        self.run_command(
            f"install-{label}",
            install_command,
            cwd=self.output,
            env=env,
            check=True,
            timeout=self.args.command_timeout_seconds,
        )

    def build_review_prompt(self) -> str:
        features = read_text(self.package_dir / "FEATURES.md")
        implementation = read_text(self.package_dir / "IMPLEMENTATION.md")
        testing = read_text(self.package_dir / "TESTING.md")
        inventory = read_text(self.package_dir / "file-inventory.txt")
        prompt = f"""
You are reviewing a migrated Divband Codex repository.

Goal:
- Check that the overlay was applied coherently to the vanilla Codex clone.
- Focus on conflicts, missing generated files, stale tests, and integration
  risks.
- Do not modify files. Return findings and suggested follow-up commands only.

Output repository:
{self.output}

Feature specification:
{features}

Implementation map:
{implementation}

Testing plan:
{testing}

File inventory:
{inventory}
"""
        return textwrap.dedent(prompt).strip()

    def run_command(
        self,
        name: str,
        command: list[str],
        cwd: Path,
        env: dict[str, str] | None = None,
        check: bool = False,
        timeout: int | None = None,
    ) -> CommandRecord:
        self.command_index += 1
        stdout_path = self.logs_dir / f"{self.command_index:03d}-{name}.stdout.log"
        stderr_path = self.logs_dir / f"{self.command_index:03d}-{name}.stderr.log"
        metadata_path = self.logs_dir / f"{self.command_index:03d}-{name}.json"
        cwd.mkdir(parents=True, exist_ok=True)
        started = time.monotonic()
        print(f"$ ({cwd}) {shlex.join(command)}")
        try:
            with stdout_path.open("w", encoding="utf-8") as stdout_file:
                with stderr_path.open("w", encoding="utf-8") as stderr_file:
                    completed = subprocess.run(
                        command,
                        cwd=cwd,
                        env=env,
                        stdout=stdout_file,
                        stderr=stderr_file,
                        text=True,
                        timeout=timeout,
                        check=False,
                    )
            duration = time.monotonic() - started
            record = CommandRecord(
                name=name,
                command=command,
                cwd=str(cwd),
                status="passed" if completed.returncode == 0 else "failed",
                returncode=completed.returncode,
                duration_seconds=duration,
                stdout=str(stdout_path.relative_to(self.manifest_dir)),
                stderr=str(stderr_path.relative_to(self.manifest_dir)),
            )
        except subprocess.TimeoutExpired:
            duration = time.monotonic() - started
            record = CommandRecord(
                name=name,
                command=command,
                cwd=str(cwd),
                status="failed",
                returncode=None,
                duration_seconds=duration,
                stdout=str(stdout_path.relative_to(self.manifest_dir)),
                stderr=str(stderr_path.relative_to(self.manifest_dir)),
                reason=f"timed out after {timeout} seconds",
            )
        self.records.append(record)
        metadata_path.write_text(json.dumps(record.as_json(), indent=2) + "\n", encoding="utf-8")
        self.write_migration_manifest(status="running")
        if check and record.status != "passed":
            raise MigrationError(f"command failed: {name}; see {metadata_path}")
        return record

    def record_skip(self, name: str, reason: str) -> None:
        record = CommandRecord(
            name=name,
            command=[],
            cwd=str(self.output),
            status="skipped",
            returncode=None,
            duration_seconds=0.0,
            reason=reason,
        )
        self.records.append(record)
        print(f"skipping {name}: {reason}")

    def write_migration_manifest(self, status: str, error: str | None = None) -> None:
        if not self.manifest_dir.exists():
            return
        manifest = {
            "status": status,
            "error": error,
            "startedAt": self.started_at,
            "updatedAt": datetime.utcnow().isoformat(timespec="seconds") + "Z",
            "packageDir": str(self.package_dir),
            "source": str(self.source),
            "output": str(self.output),
            "patchMode": self.args.patch_mode,
            "outputBranch": self.args.output_branch,
            "targetDir": str(self.target_dir) if self.target_dir else None,
            "docs": self.docs_manifest(),
            "commands": [record.as_json() for record in self.records],
        }
        (self.manifest_dir / "manifest.json").write_text(
            json.dumps(manifest, indent=2) + "\n",
            encoding="utf-8",
        )

    def write_report(self, status: str, error: str | None = None) -> None:
        if not self.manifest_dir.exists():
            return
        lines = [
            "# Divband Codex Migration Report",
            "",
            f"- Status: `{status}`",
            f"- Source: `{self.source}`",
            f"- Output: `{self.output}`",
            f"- Patch mode: `{self.args.patch_mode}`",
            f"- Test profile: `{self.args.test_profile}`",
        ]
        if error:
            lines.append(f"- Error: `{error}`")
        lines.extend(["", "## Commands", ""])
        lines.append("| Step | Status | Seconds | Log |")
        lines.append("| --- | --- | ---: | --- |")
        for record in self.records:
            log = record.stdout or record.reason or ""
            lines.append(
                f"| `{record.name}` | `{record.status}` | "
                f"{record.duration_seconds:.1f} | `{log}` |"
            )
        lines.append("")
        lines.append("## Next Steps")
        lines.append("")
        if status == "complete":
            lines.append("- Review `.divband-migration/logs/` for command output.")
            lines.append("- Push or package the generated output repository if needed.")
        else:
            lines.append("- Inspect `.divband-migration/manifest.json` and logs.")
            lines.append("- Resolve conflicts or failed commands, then rerun the orchestrator.")
        (self.manifest_dir / "REPORT.md").write_text("\n".join(lines) + "\n", encoding="utf-8")

    def docs_manifest(self) -> list[dict[str, object]]:
        docs: list[dict[str, object]] = []
        for doc_name in DOC_NAMES:
            path = self.package_dir / doc_name
            data = path.read_bytes()
            docs.append(
                {
                    "path": doc_name,
                    "sha256": hashlib.sha256(data).hexdigest(),
                    "bytes": len(data),
                }
            )
        return docs

    def patch_files(self) -> list[Path]:
        return sorted(self.patches_dir.glob("*.patch"))


def parse_args(package_dir: Path, argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Apply the Divband Codex overlay to a vanilla Codex checkout.",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    default_source = package_dir / "codex"
    default_output = package_dir.parent / "divband-codex-output"
    parser.add_argument("--source", type=Path, default=default_source)
    parser.add_argument("--output", type=Path, default=default_output)
    parser.add_argument("--patches", type=Path, default=package_dir / "patches")
    parser.add_argument("--patch-file", type=Path, default=package_dir / "divband-codex.patch")
    parser.add_argument("--patch-mode", choices=["am", "apply"], default="am")
    parser.add_argument("--output-branch", default="divband-migrated")
    parser.add_argument("--force", action="store_true", help="delete an existing output path")
    parser.add_argument("--reuse-output", action="store_true", help="reuse an existing output path")
    parser.add_argument(
        "--skip-apply",
        action="store_true",
        help="do not copy or apply patches; operate on an existing output repo",
    )
    parser.add_argument("--dry-run", action="store_true", help="print the plan without writing")
    parser.add_argument(
        "--agents",
        choices=["off", "available", "required"],
        default="available",
        help="run Codex/Cursor review agents when available",
    )
    parser.add_argument("--agent-timeout-seconds", type=int, default=900)
    parser.add_argument("--command-timeout-seconds", type=int, default=3600)
    parser.add_argument("--jobs", type=int, default=int(os.environ.get("CARGO_BUILD_JOBS", "1")))
    parser.add_argument(
        "--target-dir",
        type=Path,
        default=package_dir / ".cache" / "cargo-target",
        help="shared Cargo target dir reused across generated output repos",
    )
    parser.add_argument(
        "--no-shared-target-dir",
        action="store_true",
        help="let Cargo write target/ inside the generated output repo",
    )
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument(
        "--test-profile",
        choices=["none", "focused", "expanded"],
        default="focused",
    )
    parser.add_argument(
        "--install-tools",
        action="store_true",
        help="install missing just/cargo-nextest through cargo",
    )
    parser.add_argument(
        "--run-fix",
        action="store_true",
        help="run just fix -p codex-core after tests",
    )
    return parser.parse_args(argv)


def main(package_dir: Path, argv: list[str] | None = None) -> int:
    args = parse_args(package_dir, argv)
    runner = MigrationRunner(package_dir, args)
    return runner.run()


def safe_remove_tree(path: Path) -> None:
    resolved = path.resolve()
    if resolved == Path("/"):
        raise MigrationError("refusing to remove filesystem root")
    if len(resolved.parts) < 4:
        raise MigrationError(f"refusing to remove suspiciously broad path: {resolved}")
    shutil.rmtree(resolved)


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")
