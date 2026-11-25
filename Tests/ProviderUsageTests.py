import importlib.util
import pathlib
import unittest
from unittest.mock import patch

path = pathlib.Path(__file__).resolve().parents[1] / "integrations/Code/native_helpers/provider_usage.py"
spec = importlib.util.spec_from_file_location("provider_usage", path)
usage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(usage)

class ProviderUsageTests(unittest.TestCase):
    def test_gemini_uses_its_own_quota_and_preserves_zero_remaining(self):
        responses = [{"cloudaicompanionProject": "test-project"}, {"buckets": [{"modelId": "gemini-fixture", "remainingFraction": 0.25, "resetTime": "2026-10-28T00:00:00Z"}, {"modelId": "exhausted", "remainingFraction": 0}]}]
        with patch.object(usage, "read", return_value={"access_token": "fake-only"}), patch.object(usage, "request", side_effect=responses) as request, patch.dict(usage.os.environ, {}, clear=True):
            result = usage.collect("gemini")
        self.assertEqual([v["used_percent"] for v in result], [75, 100])
        self.assertEqual(request.call_args.args[2], {"project": "test-project"})
        self.assertIsInstance(result[0]["resets_at"], int)
        self.assertNotIn("fake-only", str(result))

    def test_opencode_uses_real_dashboard_shape_and_no_codex_windows(self):
        with patch.object(usage, "read", return_value={"opencode-go": {"key": "fake-only"}}), patch.object(usage, "request", return_value={"usage": {"rolling": {"status": "ok", "percent": 17, "resetsAt": 1800000000}, "weekly": {"status": "ok", "percent": 40}}}), patch.dict(usage.os.environ, {}, clear=True):
            result = usage.collect("opencode")
        self.assertEqual([v["used_percent"] for v in result], [17, 40])
        self.assertEqual(result[0]["duration_minutes"], 300)
        self.assertNotIn("codex", str(result).lower())

    def test_claude_keeps_model_specific_windows(self):
        with patch.object(usage.sys, "platform", "linux"), patch.object(usage, "read", return_value={"claudeAiOauth": {"accessToken": "fake-only"}}), patch.object(usage, "request", return_value={"five_hour": {"utilization": 0}, "seven_day_opus": {"utilization": 61, "resets_at": "2026-10-28T00:00:00Z"}}), patch.dict(usage.os.environ, {}, clear=True):
            result = usage.collect("claude")
        self.assertEqual([v["used_percent"] for v in result], [0, 61])
        self.assertIn("Opus", result[1]["label"])

if __name__ == "__main__":
    unittest.main()
