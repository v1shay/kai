#!/usr/bin/env python3
"""Isolated workspace transfer and inference-liveness regression checks."""
import contextlib,hashlib,importlib.util,io,json,os,pathlib,subprocess,tarfile,tempfile,time,unittest
ROOT=pathlib.Path(__file__).resolve().parents[1]
def module(name):
 spec=importlib.util.spec_from_file_location(name,ROOT/'integrations/Code/native_helpers'/f'{name}.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m
H=module('kai_ninja_handoff');S=module('kai_inference_stream')
class Handoff(unittest.TestCase):
 def setUp(self):
  self.temp=tempfile.TemporaryDirectory();self.root=pathlib.Path(self.temp.name);self.project=self.root/'project';self.project.mkdir();self.home=self.root/'home';self.home.mkdir();self.old=os.environ.get('HOME');os.environ['HOME']=str(self.home)
  self.git('init','-q');self.git('config','user.name','Validation');self.git('config','user.email','validation@example.invalid')
  (self.project/'old.txt').write_text('old');(self.project/'remove.txt').write_text('remove');(self.project/'.gitignore').write_text('.env\n');self.git('add','.');self.git('commit','-qm','initial')
 def tearDown(self):
  if self.old is None:os.environ.pop('HOME',None)
  else:os.environ['HOME']=self.old
  self.temp.cleanup()
 def git(self,*args):return subprocess.check_output(['git','-C',str(self.project),*args],stderr=subprocess.DEVNULL)
 def transfer(self,packet):
  downloads=self.home/'Downloads';downloads.mkdir(exist_ok=True);path=downloads/pathlib.Path(packet['archive']).name;path.write_bytes(pathlib.Path(packet['archive']).read_bytes());return path
 def test_snapshot_retains_history_edits_deletions_and_permissions(self):
  (self.project/'old.txt').write_text('changed');(self.project/'remove.txt').unlink();(self.project/'new.sh').write_text('#!/bin/sh\necho good\n');(self.project/'new.sh').chmod(0o755);(self.project/'.env').write_text('DO_NOT_TRANSFER')
  packet=H.export(self.project,'Quoted previous chat',self.root/'out');self.transfer(packet);receipt=H.receive(packet['id'],packet['sha256'],self.root/'state');p=pathlib.Path(receipt['project'])
  self.assertEqual((p/'old.txt').read_text(),'changed');self.assertFalse((p/'remove.txt').exists());self.assertFalse((p/'.env').exists());self.assertEqual((p/'new.sh').stat().st_mode&0o777,0o755);self.assertEqual(receipt['context'],'Quoted previous chat')
  self.assertEqual(subprocess.check_output(['git','-C',str(p),'rev-list','--count','HEAD']).strip(),b'1');status=subprocess.check_output(['git','-C',str(p),'status','--porcelain']).decode();self.assertIn('old.txt',status);self.assertIn('remove.txt',status);self.assertNotIn('.kai-handoff',status)
  self.assertEqual(H.receive(packet['id'],packet['sha256'],self.root/'state'),receipt)
 def test_checksum_and_links_are_rejected(self):
  packet=H.export(self.project,'',self.root/'out');self.transfer(packet)
  with self.assertRaises(ValueError):H.receive(packet['id'],'0'*64,self.root/'state')
  (self.project/'link').symlink_to('/etc/passwd')
  with self.assertRaises(ValueError):H.export(self.project,'',self.root/'out')
 def test_archive_traversal_rejected_even_with_matching_hash(self):
  ident='00000000-0000-4000-8000-000000000001';folder=self.home/'Downloads';folder.mkdir();p=folder/f'kai-ninja-handoff-{ident}.tar.gz'
  with tarfile.open(p,'w:gz') as tar:
   m=tarfile.TarInfo('../escape');m.size=1;tar.addfile(m,io.BytesIO(b'x'))
  with self.assertRaises(ValueError):H.receive(ident,H.digest(p),self.root/'state')
class Streaming(unittest.TestCase):
 def test_heartbeats_before_upstream_headers(self):
  class Opener:
   def open(self,*args,**kwargs):time.sleep(.06);return io.BytesIO(b'{"done":true}\n')
  stream=S.lines(Opener(),None,heartbeat=.01,deadline=1);start=time.monotonic();self.assertEqual(next(stream),b'\n');self.assertLess(time.monotonic()-start,.05);self.assertIn(b'{"done":true}\n',list(stream))
 def test_errors_propagate_after_heartbeat(self):
  class Opener:
   def open(self,*args,**kwargs):time.sleep(.03);raise ValueError('upstream failed')
  stream=S.lines(Opener(),None,heartbeat=.005,deadline=1);self.assertEqual(next(stream),b'\n')
  with self.assertRaisesRegex(ValueError,'upstream failed'):list(stream)
if __name__=='__main__':unittest.main()
