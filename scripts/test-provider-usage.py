#!/usr/bin/env python3
"""Account quota normalization tests; no credentials or network needed."""
import importlib.util, pathlib, struct, unittest
spec=importlib.util.spec_from_file_location('provider_usage',pathlib.Path(__file__).resolve().parents[1]/'integrations/Code/native_helpers/provider_usage.py')
p=importlib.util.module_from_spec(spec);spec.loader.exec_module(p)
class Quotas(unittest.TestCase):
 def test_grok_missing_percentage_is_not_zero(self):
  p.read=lambda _: {'key':'fixture'}
  p.request=lambda *a,**kw:{'config':{'currentPeriod':{'end':'2026-12-01T00:00:00Z'}}}
  self.assertEqual(p.collect('grok'),[])
 def test_grok_reported_percentage_and_reset(self):
  p.read=lambda _: {'account':{'key':'fixture'}}
  p.request=lambda *a,**kw:{'config':{'creditUsagePercent':12.5,'currentPeriod':{'end':'2026-12-01T00:00:00Z'}}}
  quota=p.collect('grok')[0]
  self.assertEqual(quota['used_percent'],12.5);self.assertIsInstance(quota['resets_at'],int)
 def test_cursor_binary_percent(self):
  self.assertEqual(p.proto(bytes([14<<3|1])+struct.pack('<d',5.))[14],5.)
 def test_cursor_rejects_truncated_field(self):
  with self.assertRaises(ValueError):p.proto(bytes([3<<3|2,100,1]))
if __name__=='__main__':unittest.main()
