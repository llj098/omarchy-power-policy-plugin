import importlib.machinery
import importlib.util
import sys
import unittest
from pathlib import Path

SCRIPT = Path(__file__).parents[1] / "bin" / "omarchy-power-policy"
LOADER = importlib.machinery.SourceFileLoader("omarchy_power_policy", str(SCRIPT))
SPEC = importlib.util.spec_from_loader(LOADER.name, LOADER)
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[LOADER.name] = MODULE
LOADER.exec_module(MODULE)
PolicyEngine = MODULE.PolicyEngine
PowerPolicyDaemon = MODULE.PowerPolicyDaemon


class FakeVariant:
    def __init__(self, value):
        self.value = value

    def unpack(self):
        return self.value


class PolicyEngineTest(unittest.TestCase):
    def test_close_on_ac_then_unplug_suspends_once(self):
        policy = PolicyEngine()
        self.assertFalse(policy.update(on_battery=False, lid_closed=False))
        self.assertFalse(policy.update(lid_closed=True))
        self.assertTrue(policy.update(on_battery=True))
        self.assertFalse(policy.update(on_battery=True))

    def test_close_while_on_battery_suspends(self):
        policy = PolicyEngine()
        self.assertFalse(policy.update(on_battery=True, lid_closed=False))
        self.assertTrue(policy.update(lid_closed=True))

    def test_open_lid_rearms_policy(self):
        policy = PolicyEngine()
        self.assertTrue(policy.update(on_battery=True, lid_closed=True))
        self.assertFalse(policy.update(lid_closed=False))
        self.assertTrue(policy.update(lid_closed=True))

    def test_plugging_ac_rearms_closed_lid(self):
        policy = PolicyEngine()
        self.assertTrue(policy.update(on_battery=True, lid_closed=True))
        self.assertFalse(policy.update(on_battery=False))
        self.assertTrue(policy.update(on_battery=True))

    def test_open_lid_unplug_does_nothing(self):
        policy = PolicyEngine()
        self.assertFalse(policy.update(on_battery=False, lid_closed=False))
        self.assertFalse(policy.update(on_battery=True))

    def test_unknown_state_never_suspends(self):
        policy = PolicyEngine()
        self.assertFalse(policy.update(lid_closed=True))
        self.assertTrue(policy.update(on_battery=True))


class UPowerSignalTest(unittest.TestCase):
    def setUp(self):
        self.daemon = object.__new__(PowerPolicyDaemon)
        self.reasons = []
        self.daemon._refresh_policy = self.reasons.append

    def test_changed_property_triggers_refresh(self):
        self.daemon._on_properties_changed(
            None, FakeVariant({"OnBattery": True}), []
        )
        self.assertEqual(self.reasons, ["upower:OnBattery"])

    def test_invalidated_lid_property_triggers_refresh(self):
        self.daemon._on_properties_changed(
            None, FakeVariant({}), ["LidIsClosed"]
        )
        self.assertEqual(self.reasons, ["upower:LidIsClosed"])

    def test_unrelated_property_is_ignored(self):
        self.daemon._on_properties_changed(
            None, FakeVariant({"DaemonVersion": "1"}), []
        )
        self.assertEqual(self.reasons, [])


if __name__ == "__main__":
    unittest.main()
