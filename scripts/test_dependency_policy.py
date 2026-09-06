# Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
# SPDX-License-Identifier: GPL-3.0-only

import copy
import tomllib
import unittest

from dependency_policy import MICA_CRATES, MICA_GIT, validate


class DependencyPolicyTests(unittest.TestCase):
    def setUp(self):
        self.revision = "a" * 40
        self.manifest = {"workspace": {"dependencies": {
            "compio": {"version": "=0.18.0"},
            **{name: {"git": MICA_GIT} for name in MICA_CRATES},
        }}}
        self.manifest["workspace"]["dependencies"]["mica-driver"].update(
            {"default-features": False, "features": ["source-provider"]}
        )
        self.lockfile = {"package": [
            {"name": "compio", "version": "0.18.0"},
            *[{"name": name, "source": f"git+{MICA_GIT}#{self.revision}"} for name in MICA_CRATES],
        ]}
        self.members = {"roe-core/Cargo.toml": {
            "dependencies": {"mica-driver": {"workspace": True}}
        }}

    def errors(self):
        return validate(self.manifest, self.lockfile, self.revision, self.members)

    def test_valid_policy_and_multiline_inheritance(self):
        self.members["roe-core/Cargo.toml"] = tomllib.loads("""
            [dependencies.mica-driver]
            # Formatting does not change policy.
            workspace = true
            features = [
                "source-provider",
            ]
        """)
        self.assertEqual(self.errors(), [])

    def test_revision_and_runtime_mismatches_have_separate_diagnostics(self):
        self.lockfile["package"][0]["version"] = "0.19.0"
        self.lockfile["package"][1]["source"] += "wrong"
        errors = self.errors()
        self.assertTrue(any("compio must resolve" in error for error in errors))
        self.assertTrue(any("mica-compiler" in error for error in errors))

    def test_target_and_dev_declarations_cannot_bypass_centralization(self):
        self.members["roe-core/Cargo.toml"] = {
            "dev-dependencies": {"compio": "0.18"},
            "target": {"cfg(unix)": {"dependencies": {"notify": "8.2"}}},
        }
        self.assertEqual(len(self.errors()), 2)

    def test_member_cannot_enable_driver_acceleration(self):
        declaration = self.members["roe-core/Cargo.toml"]["dependencies"]["mica-driver"]
        declaration.update({"default-features": True, "features": ["wgpu"]})
        self.assertEqual(len(self.errors()), 2)

    def test_lockfile_policy_rejects_a_manifest_revision_selector(self):
        self.manifest["workspace"]["dependencies"]["mica-driver"]["rev"] = self.revision
        self.assertTrue(any("remove manifest source selectors" in error for error in self.errors()))

    def test_missing_and_divergent_transitive_mica_packages_are_rejected(self):
        self.lockfile["package"] = self.lockfile["package"][:1]
        self.assertEqual(len(self.errors()), 3)
        self.setUp()
        transitive = copy.deepcopy(self.lockfile["package"][1])
        transitive.update({"name": "mica-runtime", "source": "registry+invalid"})
        self.lockfile["package"].append(transitive)
        self.assertTrue(any("mica-runtime" in error for error in self.errors()))

    def test_external_path_dependency_does_not_bypass_policy(self):
        self.members["roe-core/Cargo.toml"]["dependencies"]["mica-driver"] = {"path": "../external-mica"}
        self.assertTrue(any("workspace member" in error for error in self.errors()))

    def test_malformed_revision_is_reported(self):
        self.revision = "short"
        self.assertTrue(any("full lowercase Git commit ID" in error for error in self.errors()))


if __name__ == "__main__":
    unittest.main()
