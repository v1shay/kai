"""Real HTTP/MCP/OAuth transport tests with an isolated mock native owner."""
import base64,hashlib,http.client,importlib.util,json,pathlib,socket,tempfile,threading,unittest
module=pathlib.Path(__file__).parent/"integrations/Code/native_helpers/kai_remote.py"
spec=importlib.util.spec_from_file_location("kai_remote",module);remote=importlib.util.module_from_spec(spec);spec.loader.exec_module(remote)

class RemoteTests(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.state=remote.State(self.tmp.name)
  self.client=self.state.pair("test","control",["/project"]);self.grant=self.state.grant(self.client["token"])
  self.server=remote.Server(("127.0.0.1",0),self.state);self.server.public_url=f"http://127.0.0.1:{self.server.server_port}"
  self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
 def tearDown(self):self.server.shutdown();self.server.server_close();self.tmp.cleanup()
 def request(self,method,path,value=None,token=True,headers=None):
  connection=http.client.HTTPConnection("127.0.0.1",self.server.server_port,timeout=3)
  h={"Authorization":"Bearer "+self.client["token"]}if token else {}
  h.update(headers or {});body=json.dumps(value).encode()if value is not None else None
  connection.request(method,path,body,h);response=connection.getresponse();data=response.read();status=response.status;connection.close();return status,json.loads(data)
 def mock_host(self):
  left,right=socket.socketpair();self.state.connection=left;self.addCleanup(left.close);self.addCleanup(right.close)
  self.calls=[]
  def run():
   for line in right.makefile("rb"):
    value=json.loads(line);self.calls.append(value)
    self.state.receive({"id":value["id"],"reply":{"chat_id":"exact-chat","status":"Queued"}})
  threading.Thread(target=run,daemon=True).start()
 @unittest.skipUnless(socket.has_ipv6,"IPv6 is unavailable")
 def test_ipv6_loopback_listener_uses_a_valid_origin(self):
  server=remote.Server(("::1",0),self.state)
  thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
  try:
   self.assertEqual(server.public_url,f"http://[::1]:{server.server_port}")
   connection=http.client.HTTPConnection("::1",server.server_port,timeout=3)
   connection.request("GET","/v1/snapshot",headers={"Authorization":"Bearer "+self.client["token"]})
   response=connection.getresponse();self.assertEqual(response.status,200);response.read();connection.close()
  finally:server.shutdown();server.server_close()
 def test_auth_origin_and_scope(self):
  self.assertEqual(self.request("GET","/v1/snapshot",token=False)[0],401)
  self.assertEqual(self.request("GET","/v1/snapshot",headers={"Origin":"https://evil.test"})[0],401)
  read=self.state.pair("read");grant=self.state.grant(read["token"])
  with self.assertRaises(PermissionError):self.state.begin(grant,{"action":"create_chat","request_id":"1"})
 def test_internal_callback_fields_never_reach_the_native_owner(self):
  self.mock_host()
  request={"action":"write_artifact","request_id":"spoof-owner","project":"/project","_owner":"another-chat","_remote_id":"another-tool"}
  self.assertEqual(self.request("POST","/v1/command",request)[0],400)
  self.assertEqual(self.calls,[])
 def test_request_retries_are_durable_and_bound_to_arguments(self):
  self.mock_host();request={"action":"create_chat","request_id":"same","project":"/project","prompt":"Inspect"}
  status,result=self.request("POST","/v1/command",request);self.assertEqual(status,200);self.assertEqual(result["chat_id"],"exact-chat")
  status,result=self.request("POST","/v1/command",request);self.assertEqual(result["state"],"completed");self.assertEqual(len(self.calls),1)
  self.assertEqual(self.request("POST","/v1/command",{**request,"prompt":"Different"})[0],400)
  self.assertEqual(self.request("GET","/v1/receipts/same")[1]["state"],"completed")
 def test_unknown_result_does_not_replay_after_restart(self):
  request={"action":"create_chat","request_id":"ambiguous"};self.state.begin(self.grant,request)
  with self.state.sql() as db:db.execute("UPDATE receipts SET state='uncertain'")
  restored=remote.State(self.tmp.name);result=restored.begin(self.grant,request)
  self.assertEqual(result["state"],"uncertain")
 def test_mcp_discovery_and_invocation_use_same_host(self):
  self.mock_host()
  status,result=self.request("POST","/mcp",{"jsonrpc":"2.0","id":1,"method":"initialize"});self.assertEqual(result["result"]["serverInfo"]["name"],"Kai Unified")
  status,result=self.request("POST","/mcp",{"jsonrpc":"2.0","id":2,"method":"tools/list"});self.assertEqual(result["result"]["tools"][0]["name"],"kai_workspace")
  status,result=self.request("POST","/mcp",{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"kai_workspace","arguments":{"action":"list_projects"}}})
  self.assertFalse(result["result"]["isError"]);self.assertEqual(len(self.calls),1)
 def test_scoped_snapshots_never_leak_other_projects(self):
  value={"projects":[{"path":"/project"},{"path":"/secret"}],"tasks":[{"project":"/secret","prompt":"secret"}],"draft":"secret","selected_chat":"secret"}
  result=remote.scoped_snapshot(value,self.grant);self.assertEqual(result["projects"],[{"path":"/project"}]);self.assertEqual(result["tasks"],[]);self.assertNotIn("draft",result)
 def test_sse_reconnect_has_epoch_sequence_and_authoritative_snapshot(self):
  self.state.receive({"snapshot":{"connected":True,"tasks":[{"project":"/project","chat_id":"x"}]}})
  c=http.client.HTTPConnection("127.0.0.1",self.server.server_port,timeout=3);c.request("GET","/v1/events",headers={"Authorization":"Bearer "+self.client["token"],"Last-Event-ID":"old-epoch:5"});r=c.getresponse()
  self.assertEqual(r.status,200);self.assertIn(self.state.epoch,r.fp.readline().decode());self.assertIn(b"snapshot",r.fp.readline());self.assertIn(b'"chat_id":"x"',r.fp.readline());c.close()
 def test_oauth_pkce_and_one_time_pairing(self):
  status,value=self.request("POST","/register",{"client_name":"ChatGPT test","redirect_uris":["https://example.test/callback"]},token=False);self.assertEqual(status,201);client=value["client_id"]
  self.assertEqual(self.request("POST","/register",{"redirect_uris":["http://evil.test/"]},token=False)[0],400)
  code="one-time-pair-code";verifier="a"*43;challenge=base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
  with self.state.sql() as db:db.execute("INSERT INTO pairing VALUES(?,?,?,?)",(remote.digest(code),"read",'["/project"]',remote.time.time()+60))
  def form(path,fields):
   c=http.client.HTTPConnection("127.0.0.1",self.server.server_port,timeout=3);c.request("POST",path,remote.urllib.parse.urlencode(fields),{"Content-Type":"application/x-www-form-urlencoded"});r=c.getresponse();result=(r.status,r.getheader("Location"),r.read());c.close();return result
  fields={"client_id":client,"redirect_uri":"https://example.test/callback","pairing_code":code,"response_type":"code","code_challenge_method":"S256","code_challenge":challenge,"scope":"read","state":"original"}
  status,location,_=form("/authorize",fields);self.assertEqual(status,302);auth=remote.urllib.parse.parse_qs(remote.urllib.parse.urlsplit(location).query)["code"][0]
  self.assertEqual(form("/authorize",fields)[0],401)
  fields={"grant_type":"authorization_code","code":auth,"code_verifier":verifier,"client_id":client,"redirect_uri":"https://example.test/callback"}
  status,_,body=form("/token",fields);self.assertEqual(status,200);token=json.loads(body)["access_token"];self.assertEqual(self.state.grant(token)["scope"],"read");self.assertEqual(form("/token",fields)[0],401)
 def test_mcp_apps_resource_is_negotiated_self_contained_and_scoped(self):
  call=lambda method,params={}:self.request("POST","/mcp",{"jsonrpc":"2.0","id":1,"method":method,"params":params})[1]["result"]
  self.assertNotIn("_meta",call("tools/list")["tools"][0])
  call("initialize",{"protocolVersion":"2025-06-18","capabilities":{"extensions":{"io.modelcontextprotocol/ui":{"mimeTypes":["text/html;profile=mcp-app"]}}}})
  tool=call("tools/list")["tools"][0];self.assertEqual(tool["_meta"]["ui"]["resourceUri"],"ui://kai/workspace")
  resource=call("resources/read",{"uri":"ui://kai/workspace"})["contents"][0]
  self.assertEqual(resource["mimeType"],"text/html;profile=mcp-app");self.assertEqual(resource["_meta"]["ui"]["csp"]["connectDomains"],[])
  self.assertTrue(resource["text"].startswith("<!doctype html>"));self.assertNotIn('<script src=',resource["text"])
  self.state.snapshot={"protocol":"1","connected":True,"tasks":[{"project":"/secret"}]}
  snapshot=call("tools/call",{"name":"kai_workspace","arguments":{"action":"snapshot"}})
  self.assertEqual(json.loads(snapshot["content"][0]["text"])["tasks"],[])
 def test_receipts_are_semantic_and_never_cross_clients(self):
  self.mock_host();self.request("POST","/v1/command",{"action":"create_chat","project":"/project","request_id":"ordered"})
  status,value=self.request("POST","/v1/command",{"request_id":"ordered","project":"/project","action":"create_chat"});self.assertEqual(status,200);self.assertEqual(value["state"],"completed");self.assertEqual(len(self.calls),1)
  second=self.state.pair("second","control");c=http.client.HTTPConnection("127.0.0.1",self.server.server_port,timeout=3);c.request("GET","/v1/receipts/ordered",headers={"Authorization":"Bearer "+second["token"]});r=c.getresponse();self.assertEqual(r.status,404);r.read();c.close()
 def test_replay_memory_has_a_byte_budget(self):
  for i in range(30):self.state.receive({"snapshot":{"value":"x"*300000,"revision":i}})
  self.assertLessEqual(self.state.event_bytes,4*1024*1024);self.assertLess(len(self.state.events),30)
 def test_host_and_oauth_origin_are_validated(self):
  self.assertEqual(self.request("GET","/v1/snapshot",headers={"Host":"evil.test"})[0],401)
  self.assertEqual(self.request("POST","/register",{"redirect_uris":["https://example.test/callback"]},False,{"Origin":"https://evil.test"})[0],401)
 def test_scoped_catalogs_and_unknown_actions(self):
  snapshot={"catalogs":[{"project":"/project","models":["a"]},{"project":"/secret","models":["b"]}]}
  self.assertEqual(remote.scoped_snapshot(snapshot,self.grant)["catalogs"],[snapshot["catalogs"][0]])
  self.assertEqual(self.request("POST","/v1/command",{"action":"raw_shell","request_id":"no"})[0],400)
 def test_revocation_applies_to_existing_credentials(self):
  with self.state.sql() as db:db.execute("DELETE FROM clients WHERE id=?",(self.grant["id"],))
  with self.assertRaises(PermissionError):self.state.grant(self.client["token"])

if __name__=="__main__":unittest.main()
