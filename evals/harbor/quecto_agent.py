"""
QuECTO Harbor Agent Adapter (Harbor >= 0.22)
============================================
Installs a Linux build of `quecto-agent` into the task container and runs it
non-interactively against an OpenAI-compatible endpoint (local Ollama by
default).

Usage:
    QUECTO_AGENT_BIN=/path/to/quecto-agent-linux \\
    QUECTO_MODEL=qwen3.6:35b-a3b-coding-mxfp8 \\
    QUECTO_BASE_URL=http://<ollama-host>:11434/v1 \\
    harbor run -d terminal-bench/terminal-bench-2 \\
      --agent-import-path evals.harbor.quecto_agent:QuectoAgent

The binary must match the container architecture (Linux). Build one with:
    docker run --rm -v "$PWD":/src -w /src rust:1-bookworm \\
        cargo build --release -p quecto-agent

Ablation switches are forwarded from the environment so arms can be run
through Harbor as well:
    QUECTO_AGENT_ARGS="--no-verify --no-seed-context --no-session"
    QUECTO_REASONING_MODE=none
"""

from __future__ import annotations

import os
import shlex
from pathlib import Path
from typing import override

from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

REMOTE_BIN = "/usr/local/bin/quecto-agent"

# Environment variables forwarded into the container (names only; values are
# read from the process running `harbor`, never hard-coded).
FORWARDED_ENV = (
    "QUECTO_MODEL",
    "QUECTO_BASE_URL",
    "QUECTO_API_KEY",
    "QUECTO_PROVIDER",
    "QUECTO_REASONING_MODE",
    "QUECTO_MAX_STEPS",
    "QUECTO_VERIFY",
)


class QuectoAgent(BaseInstalledAgent):
    """Harbor adapter for quecto-agent."""

    SUPPORTS_RESUME: bool = False

    @staticmethod
    @override
    def name() -> str:
        return "quecto-agent"

    @override
    def get_version_command(self) -> str | None:
        return f"{REMOTE_BIN} --version"

    @override
    def parse_version(self, stdout: str) -> str:
        return stdout.strip().removeprefix("quecto-agent").strip()

    @override
    async def install(self, environment: BaseEnvironment) -> None:
        binary = Path(os.environ.get("QUECTO_AGENT_BIN", "")).expanduser()
        if not binary.is_file():
            raise FileNotFoundError(
                "Set QUECTO_AGENT_BIN to a Linux build of quecto-agent "
                f"(got {str(binary)!r})."
            )
        await environment.upload_file(binary, "/tmp/quecto-agent")
        await self.exec_as_root(
            environment,
            command=f"install -m 0755 /tmp/quecto-agent {REMOTE_BIN} && {REMOTE_BIN} --version",
        )

    @override
    def populate_context_post_run(self, context: AgentContext) -> None:
        pass

    @override
    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        env = {k: os.environ[k] for k in FORWARDED_ENV if k in os.environ}
        extra = os.environ.get("QUECTO_AGENT_ARGS", "")
        await self.exec_as_agent(
            environment,
            command=(
                "mkdir -p /logs/agent; "
                f"QUECTO_TRACE_FILE=/logs/agent/trace.jsonl "
                f"{REMOTE_BIN} --yes {extra} {shlex.quote(instruction)} "
                "2>&1 | tee /logs/agent/quecto.txt"
            ),
            env=env,
        )
