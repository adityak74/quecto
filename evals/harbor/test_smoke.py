#!/usr/bin/env python3
"""
Smoke test for evals/harbor/quecto_agent.py

Tests the adapter logic without requiring the real Harbor SDK or a live
quecto-agent by stubbing both dependencies.

Run from the repo root:
    python evals/harbor/test_smoke.py
"""
from __future__ import annotations

import os
import sys
import shutil
import tempfile
import subprocess
import types
import unittest
from pathlib import Path
from unittest.mock import patch, MagicMock

# ── Stub the Harbor SDK so import succeeds without `pip install harbor` ──────
_harbor_stub = types.ModuleType("harbor")
_base_stub = types.ModuleType("harbor.agents")
_installed_stub = types.ModuleType("harbor.agents.installed")
_base_module = types.ModuleType("harbor.agents.installed.base")

class _BaseInstalledAgent:
    name: str = ""
    description: str = ""
    def install(self) -> None: ...
    def run(self, task, **kwargs) -> str: ...

class _TaskEnvironment:
    def __init__(self, instruction: str, workdir: str):
        self.instruction = instruction
        self.workdir = workdir

_base_module.BaseInstalledAgent = _BaseInstalledAgent
_base_module.TaskEnvironment = _TaskEnvironment
sys.modules["harbor"] = _harbor_stub
sys.modules["harbor.agents"] = _base_stub
sys.modules["harbor.agents.installed"] = _installed_stub
sys.modules["harbor.agents.installed.base"] = _base_module

# Now we can import the adapter safely
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import importlib
import evals.harbor.quecto_agent as adapter_module

AGENT_BIN = str(
    Path(__file__).resolve().parents[2] / "target" / "release" / "quecto-agent"
)

class TestQuectoAgentAdapter(unittest.TestCase):

    # ── install() ─────────────────────────────────────────────────────────────

    def test_install_raises_when_binary_missing(self):
        """install() should raise FileNotFoundError if the binary is absent."""
        agent = adapter_module.QuectoAgent()
        with patch.object(adapter_module, "QUECTO_AGENT_BIN", "/nonexistent/quecto-agent"):
            with self.assertRaises(FileNotFoundError):
                agent.install()

    def test_install_succeeds_when_binary_exists(self):
        """install() should pass silently when the binary is present."""
        agent = adapter_module.QuectoAgent()
        # Use any real executable as a stand-in
        real_bin = shutil.which("bash") or "/bin/bash"
        with patch.object(adapter_module, "QUECTO_AGENT_BIN", real_bin):
            agent.install()   # must not raise

    # ── run() ─────────────────────────────────────────────────────────────────

    def test_run_returns_stdout_on_success(self):
        """run() should return the agent's stdout when it exits 0."""
        agent = adapter_module.QuectoAgent()
        task = _TaskEnvironment(
            instruction="Write hello.txt containing the word 'hello'.",
            workdir=tempfile.mkdtemp(),
        )
        fake_result = MagicMock()
        fake_result.returncode = 0
        fake_result.stdout = "Agent ran successfully.\n"
        fake_result.stderr = ""

        with patch("subprocess.run", return_value=fake_result) as mock_run:
            with patch.object(adapter_module, "QUECTO_AGENT_BIN", "/fake/quecto-agent"):
                output = agent.run(task)

        self.assertEqual(output, "Agent ran successfully.\n")
        mock_run.assert_called_once()
        call_args = mock_run.call_args
        self.assertIn("--yes", call_args[0][0])
        self.assertIn(task.instruction, call_args[0][0])
        self.assertEqual(call_args[1]["cwd"], task.workdir)

    def test_run_returns_stderr_on_failure(self):
        """run() should surface stderr when the agent exits non-zero."""
        agent = adapter_module.QuectoAgent()
        task = _TaskEnvironment(instruction="Fail please.", workdir=tempfile.mkdtemp())
        fake_result = MagicMock()
        fake_result.returncode = 1
        fake_result.stdout = ""
        fake_result.stderr = "some error from agent\n"

        with patch("subprocess.run", return_value=fake_result):
            with patch.object(adapter_module, "QUECTO_AGENT_BIN", "/fake/quecto-agent"):
                output = agent.run(task)

        self.assertIn("[quecto-agent exited 1]", output)
        self.assertIn("some error from agent", output)

    def test_run_sets_env_vars(self):
        """run() must forward QUECTO_MODEL and QUECTO_BASE_URL."""
        agent = adapter_module.QuectoAgent()
        task = _TaskEnvironment(instruction="test", workdir=tempfile.mkdtemp())
        fake_result = MagicMock(returncode=0, stdout="ok", stderr="")

        env_override = {
            "QUECTO_MODEL": "phi4:latest",
            "QUECTO_BASE_URL": "http://custom:8080/v1",
        }
        with patch.dict(os.environ, env_override, clear=False):
            with patch("subprocess.run", return_value=fake_result) as mock_run:
                with patch.object(adapter_module, "QUECTO_AGENT_BIN", "/fake/quecto-agent"):
                    agent.run(task)

        passed_env = mock_run.call_args[1]["env"]
        self.assertEqual(passed_env["QUECTO_MODEL"], "phi4:latest")
        self.assertEqual(passed_env["QUECTO_BASE_URL"], "http://custom:8080/v1")

    # ── Integration: adapter against real binary (skipped if binary absent) ──

    @unittest.skipUnless(Path(AGENT_BIN).is_file(), "quecto-agent binary not built")
    def test_integration_real_binary(self):
        """
        Integration smoke test: adapter invokes real quecto-agent --yes.
        Uses a simple prompt that should complete quickly.
        """
        agent = adapter_module.QuectoAgent()
        workdir = tempfile.mkdtemp()
        task = _TaskEnvironment(
            instruction="Create a file named integration_test.txt containing the text 'smoke-ok'.",
            workdir=workdir,
        )
        with patch.object(adapter_module, "QUECTO_AGENT_BIN", AGENT_BIN):
            output = agent.run(task)

        result_file = Path(workdir) / "integration_test.txt"
        self.assertTrue(
            result_file.exists(),
            f"Expected integration_test.txt in {workdir}. Agent output:\n{output}",
        )
        self.assertIn("smoke-ok", result_file.read_text())


if __name__ == "__main__":
    unittest.main(verbosity=2)
