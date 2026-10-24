#!/usr/bin/env python3
"""
Smoke test for evals/harbor/quecto_agent.py (Harbor >= 0.22).

Needs Harbor importable. If you installed it with `uv tool install harbor`:

    PYTHONPATH=. ~/.local/share/uv/tools/harbor/bin/python evals/harbor/test_smoke.py

It exercises install() and run() against a fake environment; no container,
model, or real quecto-agent binary is involved.
"""
from __future__ import annotations

import asyncio
import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from harbor.agents.installed.base import BaseInstalledAgent

import evals.harbor.quecto_agent as adapter


class FakeEnv:
    default_user = None

    def __init__(self):
        self.uploads = []

    async def upload_file(self, source, target):
        self.uploads.append((str(source), target))


def make_agent():
    agent = adapter.QuectoAgent.__new__(adapter.QuectoAgent)
    agent.calls = []

    async def fake_root(environment, command, **kw):
        agent.calls.append(("root", command, kw))
        return SimpleNamespace(return_code=0, stdout="", stderr="")

    async def fake_agent(environment, command, **kw):
        agent.calls.append(("agent", command, kw))
        return SimpleNamespace(return_code=0, stdout="", stderr="")

    agent.exec_as_root = fake_root
    agent.exec_as_agent = fake_agent
    return agent


class TestQuectoAgentAdapter(unittest.TestCase):
    def test_is_a_harbor_installed_agent(self):
        self.assertTrue(issubclass(adapter.QuectoAgent, BaseInstalledAgent))
        self.assertEqual(adapter.QuectoAgent.name(), "quecto-agent")

    def test_install_requires_binary(self):
        with patch.dict(os.environ, {"QUECTO_AGENT_BIN": "/nonexistent/quecto"}):
            with self.assertRaises(FileNotFoundError):
                asyncio.run(make_agent().install(FakeEnv()))

    def test_install_uploads_and_installs_binary(self):
        with tempfile.NamedTemporaryFile() as f, patch.dict(
            os.environ, {"QUECTO_AGENT_BIN": f.name}
        ):
            agent, env = make_agent(), FakeEnv()
            asyncio.run(agent.install(env))
            self.assertEqual(env.uploads, [(str(Path(f.name)), "/tmp/quecto-agent")])
            self.assertIn(adapter.REMOTE_BIN, agent.calls[0][1])

    def test_run_forwards_env_flags_and_quotes_instruction(self):
        env_vars = {
            "QUECTO_MODEL": "m",
            "QUECTO_BASE_URL": "http://h/v1",
            "QUECTO_AGENT_ARGS": "--no-verify --no-session",
        }
        with patch.dict(os.environ, env_vars):
            agent = make_agent()
            agent._prompt_template_path = None
            asyncio.run(agent.run("fix 'it'", FakeEnv(), SimpleNamespace()))
        who, cmd, kw = agent.calls[0]
        self.assertEqual(who, "agent")
        self.assertIn("--yes --no-verify --no-session", cmd)
        self.assertIn("QUECTO_TRACE_FILE=/logs/agent/trace.jsonl", cmd)
        self.assertEqual(kw["env"]["QUECTO_MODEL"], "m")
        self.assertEqual(kw["env"]["QUECTO_BASE_URL"], "http://h/v1")
        self.assertNotIn("QUECTO_API_KEY", kw["env"])


if __name__ == "__main__":
    unittest.main()
