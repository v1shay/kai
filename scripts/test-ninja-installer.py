#!/usr/bin/env python3
"""Check worker replacement against isolated Unix sockets, never user services."""
import importlib.util,json,pathlib,plistlib,subprocess,sys,tempfile,time,unittest
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('installer',pathlib.Path(__file__).with_name('install-ninja-worker.py'));installer=importlib.util.module_from_spec(spec);spec.loader.exec_module(installer)
SERVER='''import socket,sys,json
s=socket.socket(socket.AF_UNIX);s.bind(sys.argv[1]);s.listen()
while True:
 c,_=s.accept();c.recv(65536)
 try:c.sendall(json.dumps({'tasks':[{'state':'running'}] if sys.argv[2]=='busy' else []}).encode()+b'\\n')
 except OSError:pass
 c.close()
'''
class WorkerReplacement(unittest.TestCase):
 def setUp(self):
  self.temp=tempfile.TemporaryDirectory(prefix='kai-worker-update-');self.address=str(pathlib.Path(self.temp.name)/'worker.sock');self.process=None
 def tearDown(self):
  if self.process and self.process.poll() is None:self.process.terminate();self.process.wait(timeout=3)
  self.temp.cleanup()
 def start(self,busy=False):
  self.process=subprocess.Popen([sys.executable,'-c',SERVER,self.address,'busy' if busy else 'idle','--ninja-worker'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  for _ in range(100):
   if pathlib.Path(self.address).exists():return
   time.sleep(.01)
  self.fail('Fixture worker did not start')
 def test_exact_idle_socket_owner_retires(self):
  self.start();self.assertEqual(installer.worker_state(self.address),{'tasks':[]});installer.retire_idle_worker(self.address);self.process.wait(timeout=3);self.assertIsNone(installer.worker_state(self.address))
 def test_active_worker_is_left_running(self):
  self.start(True)
  with self.assertRaisesRegex(ValueError,'Finish or stop'):installer.retire_idle_worker(self.address)
  self.assertIsNone(self.process.poll())
 def test_absent_worker_needs_no_process_action(self):
  self.assertIsNone(installer.worker_state(self.address));installer.retire_idle_worker(self.address)
class PortableHost(unittest.TestCase):
 def test_first_install_uses_this_hosts_tailnet_identity(self):
  with tempfile.TemporaryDirectory() as folder,patch.object(installer.shutil,'which',return_value='/test/bin/tailscale'),patch.object(installer.subprocess,'check_output',return_value=json.dumps({'Self':{'UserID':72},'User':{'72':{'LoginName':'new-host@example.invalid'}}})):
   config=installer.host_config(pathlib.Path(folder))
   self.assertEqual(config['owner'],'new-host@example.invalid');self.assertEqual(config['identity'],'whois')
 def test_runtime_update_preserves_permission_bearing_bundle(self):
  with tempfile.TemporaryDirectory() as folder:
   root=pathlib.Path(folder);home=root/'another-user';home.mkdir();bundle=root/'Kai.app';resources=bundle/'Contents/Resources';resources.mkdir(parents=True);binary=bundle/'Contents/MacOS/kai-unified';binary.parent.mkdir();binary.write_bytes(b'first runtime')
   for name in ['kai-ninja-host','kai_ninja.py','kai_inference_stream.py','kai_ninja_handoff.py','kai_workspace_mcp.py','kai_ninja_host.py']:(resources/name).write_text('fixture '+name)
   (bundle/'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleVersion':'1','CFBundleShortVersionString':'1'}))
   with patch.object(installer.pathlib.Path,'home',return_value=home),patch.object(installer,'worker_state',return_value=None),patch.object(installer,'host_config',return_value={'owner':'new@example.invalid','port':32187}),patch.object(installer.subprocess,'run',return_value=subprocess.CompletedProcess([],0)) as run:
    installer.install(bundle,False)
    host=home/'.kai-ninja/Worker.app';before={str(p.relative_to(host)):p.read_bytes() for p in host.rglob('*') if p.is_file()}
    binary.write_bytes(b'updated runtime');(resources/'kai-ninja-host').write_bytes(b'new build of launcher must not overwrite granted host')
    installer.install(bundle,False)
    after={str(p.relative_to(host)):p.read_bytes() for p in host.rglob('*') if p.is_file()};self.assertEqual(before,after)
    self.assertEqual((home/'.kai-ninja/Runtime.app/Contents/MacOS/kai-unified').read_bytes(),b'updated runtime')
    service=plistlib.loads((home/'Library/LaunchAgents/com.kai.ninjas.worker.plist').read_bytes())
    self.assertEqual(service['ProgramArguments'],[str(host/'Contents/MacOS/kai-ninja-host')])
    self.assertIn('com.kai.ninjas.worker',service['AssociatedBundleIdentifiers'])
    self.assertTrue(any('bootout' in call.args[0] for call in run.call_args_list))
if __name__=='__main__':unittest.main()
