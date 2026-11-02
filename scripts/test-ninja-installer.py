#!/usr/bin/env python3
"""Check worker replacement against isolated Unix sockets, never user services."""
import importlib.util,json,pathlib,subprocess,sys,tempfile,time,unittest
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
if __name__=='__main__':unittest.main()
