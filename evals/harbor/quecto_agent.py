"""
QuECTO Harbor Agent Adapter
===========================
Wraps `quecto-agent` as a Harbor-compatible BaseInstalledAgent so that
Terminal-Bench tasks can be evaluated against QuECTO through the Harbor
framework (https://harborframework.com).

Usage:
    pip install harbor
    harbor run \
      -d terminal-bench/terminal-bench-2 \
      -m qwen3.6:35b-mlx \
      --agent evals.harbor.quecto_agent:QuectoAgent

Requirements:
    pip install harbor            # Harbor SDK
    # quecto-agent binary must be on PATH or set QUECTO_AGENT_BIN env var
"""

from __future__ import annotations

import os
import subprocess
import shutil
from pathlib import Path
from typing import Any

# Harbor SDK — install via `pip install harbor`
try:
    from harbor.agents.installed.base import BaseInstalledAgent, TaskEnvironment
except ImportError:
    raise ImportError(
        "Harbor SDK not installed. Run: pip install harbor"
    )

# Path to quecto-agent binary — override with QUECTO_AGENT_BIN env var
_DEFAULT_BIN = str(
    Path(__file__).resolve().parents[2] / "target" / "release" / "quecto-agent"
)
QUECTO_AGENT_BIN = os.environ.get("QUECTO_AGENT_BIN", _DEFAULT_BIN)


class QuectoAgent(BaseInstalledAgent):
    """
    Harbor agent adapter for quecto-agent.

    The agent binary is run non-interactively (--yes) inside the task's
    working directory. Model and endpoint are configurable via environment
    variables that `quecto-agent` already reads natively:

        QUECTO_MODEL        — model identifier (default: qwen3.6:35b-mlx)
        QUECTO_BASE_URL     — OpenAI-compat API base URL (default: http://localhost:11434/v1)
        QUECTO_API_KEY      — API key if required

    Harbor passes the task instruction as `task.instruction` and the
    working directory as `task.workdir`.
    """

    name = "quecto-agent"
    description = (
        "QuECTO coding agent — a 3.5 MB statically-linked harness "
        "that routes to any OpenAI-compatible local or remote model."
    )

    def install(self) -> None:
        """Verify the binary exists and is executable."""
        if not shutil.which(QUECTO_AGENT_BIN) and not Path(QUECTO_AGENT_BIN).is_file():
            raise FileNotFoundError(
                f"quecto-agent binary not found at {QUECTO_AGENT_BIN}. "
                "Build it with: cargo build --release -p quecto-agent"
            )

    def run(self, task: TaskEnvironment, **kwargs: Any) -> str:
        """
        Execute quecto-agent with the task instruction in the task workdir.
        Returns the agent's stdout as the trajectory string.
        """
        env = {**os.environ}
        env["QUECTO_MODEL"] = os.environ.get("QUECTO_MODEL", "qwen3.6:35b-mlx")
        env["QUECTO_BASE_URL"] = os.environ.get("QUECTO_BASE_URL", "http://localhost:11434/v1")

        result = subprocess.run(
            [QUECTO_AGENT_BIN, "--yes", task.instruction],
            cwd=task.workdir,
            env=env,
            capture_output=True,
            text=True,
        )

        if result.returncode != 0:
            # Return stderr so Harbor can surface failure details
            return f"[quecto-agent exited {result.returncode}]\n{result.stderr}"

        return result.stdout
