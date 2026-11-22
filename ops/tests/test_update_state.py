import contextlib
import errno
import importlib.util
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("updater", Path(__file__).resolve().parents[1] / "update-panel.py")
updater = importlib.util.module_from_spec(spec)
spec.loader.exec_module(updater)


class StateSwitch(unittest.TestCase):
    def setUp(self):
        self.stack = contextlib.ExitStack()
        self.addCleanup(self.stack.close)
        self.target = Path(self.stack.enter_context(tempfile.TemporaryDirectory()))
        self.source = Path(self.stack.enter_context(tempfile.TemporaryDirectory()))
        (self.target / "old").write_text("live")
        (self.source / "new").write_text("snapshot")

    def switch(self):
        updater.replace_contents(self.target, self.source, os.getuid(), os.getgid())

    def test_same_volume(self):
        self.switch()
        self.assertEqual((self.target / "new").read_text(), "snapshot")
        self.assertFalse((self.target / "old").exists())

    def test_other_volume(self):
        if not Path("/dev/shm").is_dir():
            self.skipTest("no second filesystem")
        self.source = Path(self.stack.enter_context(tempfile.TemporaryDirectory(dir="/dev/shm")))
        if self.source.stat().st_dev == self.target.stat().st_dev:
            self.skipTest("same filesystem")
        (self.source / "new").write_text("snapshot")
        self.switch()
        self.assertEqual((self.target / "new").read_text(), "snapshot")
        self.assertFalse((self.target / "old").exists())
        self.assertEqual((self.source / "new").read_text(), "snapshot")

    def test_copy_full_preserves_live_state(self):
        with patch.object(updater.shutil, "copyfileobj", side_effect=OSError(errno.ENOSPC,"full")):
            with self.assertRaises(OSError):
                self.switch()
        self.assertEqual(list(self.target.iterdir()), [self.target / "old"])
        self.assertEqual((self.target / "old").read_text(), "live")

    def test_partial_switch_restores_old_files(self):
        original = os.rename
        (self.source / "second").write_text("data")
        installed = []
        def rename(source, target):
            if source.parent.name == "incoming":
                if installed:
                    raise OSError("switch interrupted")
                installed.append(source.name)
            original(source, target)
        with patch.object(updater.os, "rename", side_effect=rename):
            with self.assertRaises(OSError):
                self.switch()
        self.assertEqual(list(self.target.iterdir()), [self.target / "old"])
        self.assertEqual((self.target / "old").read_text(), "live")

    def test_failed_health_restores_program_and_state_across_volumes(self):
        if os.geteuid() or not Path("/dev/shm").is_dir():
            self.skipTest("simulated root updater requires a second volume")
        data = Path(self.stack.enter_context(tempfile.TemporaryDirectory(dir="/dev/shm")))
        if data.stat().st_dev == self.target.stat().st_dev:
            self.skipTest("same filesystem")
        root = self.target / "program"
        (root / "bin").mkdir(parents=True)
        binary = root / "bin" / ("probe-linux-arm64" if os.uname().machine == "aarch64" else "probe-linux-amd64")
        binary.write_text("#!/bin/sh\necho probe 1.0.1\n")
        binary.chmod(0o755)
        state = data / "control"
        state.mkdir()
        (state / "data").write_text("current")
        work = self.target / "updater" / "test"
        (work / "previous-program").mkdir(parents=True)
        (work / "previous-program" / "replacement").touch()
        previous = work / "previous-state" / "control"
        previous.mkdir(parents=True)
        (previous / "data").write_text("previous")
        updater.atomic(work / "previous.json", {"version":"1.0.0"})
        cfg = dict(name="test",root=str(root),state=str(state),uid=os.getuid(),gid=os.getgid())
        def unhealthy(cfg, version):
            self.assertEqual((state / "data").read_text(), "previous")
            self.assertTrue((root / "replacement").exists())
            (state / "data").write_text("changed by rejected new service")
            raise ValueError("health check failed")
        with patch.object(updater, "BASE", work.parent), patch.object(updater, "select_layout"), patch.object(updater, "relabel"), patch.object(updater, "service") as service, patch.object(updater, "check", side_effect=unhealthy):
            with self.assertRaisesRegex(ValueError,"health check failed"):
                updater.apply(cfg,dict(action="rollback",at=updater.time.time()))
            self.assertEqual([call.args[1] for call in service.call_args_list],["stop","start","stop","start"])
        self.assertEqual((state / "data").read_text(), "current")
        self.assertTrue(binary.exists())
        self.assertEqual((previous / "data").read_text(), "previous")


    def test_archive_preparation_failure_keeps_existing_rollback(self):
        work = self.target
        prior = work / "previous-state"
        prior.mkdir()
        (prior / "checkpoint").write_text("existing rollback")
        snap = self.source / "snapshot"
        snap.mkdir()
        with patch.object(updater.shutil, "copytree", side_effect=OSError(errno.ENOSPC, "full")):
            with self.assertRaises(OSError):
                updater.save_previous(work, self.source, snap, "1.2.3")
        self.assertEqual((prior / "checkpoint").read_text(), "existing rollback")
        self.assertFalse(list(work.glob(".yuji-update-*")))
