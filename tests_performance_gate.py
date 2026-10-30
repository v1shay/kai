import importlib.util,pathlib,unittest,json,tempfile,shutil
from unittest import mock
spec=importlib.util.spec_from_file_location("gate",pathlib.Path(__file__).parent/"scripts/check-release-performance.py");gate=importlib.util.module_from_spec(spec);spec.loader.exec_module(gate)
class GateTests(unittest.TestCase):
 def test_interrupted_collector_replaces_a_stale_passing_certificate(self):
  source=pathlib.Path(__file__).parent
  spec=importlib.util.spec_from_file_location("collector",source/"scripts/collect-release-performance.py");collector=importlib.util.module_from_spec(spec);spec.loader.exec_module(collector)
  with tempfile.TemporaryDirectory()as folder:
   root=pathlib.Path(folder);bundle=root/".build/unified/Kai Unified.app/Contents"
   for name in ["MacOS/kai-unified","Resources/libKaiNotch.dylib"]:
    path=bundle/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(b"isolated candidate")
   (root/"scripts").mkdir();shutil.copy2(source/"scripts/check-release-performance.py",root/"scripts/check-release-performance.py")
   reports=root/".build/validation";reports.mkdir();(reports/"release-performance-gate.json").write_text('{"passed":true}')
   with mock.patch.object(collector,"root",root),mock.patch.object(collector,"reports",reports),mock.patch.object(collector,"probe",side_effect=RuntimeError("controlled interruption")):
    self.assertEqual(collector.main(),1)
   report=json.loads((reports/"release-performance-gate.json").read_text());self.assertFalse(report["passed"]);self.assertIn("controlled interruption",report["reason"])
 def test_missing_evidence_never_passes(self):self.assertFalse(gate.evaluate({})["passed"])
 def test_sufficient_samples_and_outliers(self):
  evidence={name:[limit/2]*30 for name,limit in gate.LIMITS.items()};self.assertTrue(gate.evaluate(evidence)["passed"])
  evidence["warm_chat_switch_ms_p95"]=[2]*20+[200]*3;self.assertFalse(gate.evaluate(evidence)["passed"])
 def test_nan_and_tiny_fixture_do_not_certify(self):
  for bad in [[float("nan")]*30,[0.01]]:self.assertFalse(gate.evaluate({"warm_chat_switch_ms_p95":bad})["passed"])
if __name__=="__main__":unittest.main()
