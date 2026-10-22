import unittest
from kai import list_thread_rows

class CodeDiscoveryTests(unittest.TestCase):
    def test_code_exec_sessions_are_requested_on_every_page(self):
        class Server:
            def call(self, method, params, timeout):
                self.assertions.append(params)
                return {"data": [{"id": "code-agent", "source": "exec"}] if params.get("cursor") else [], "nextCursor": None if params.get("cursor") else "second"}
        server=Server();server.assertions=[]
        rows,more=list_thread_rows(server)
        self.assertEqual(rows[0]["id"],"code-agent")
        self.assertFalse(more)
        for params in server.assertions:
            self.assertEqual(params["sourceKinds"],["cli","vscode","exec"])

if __name__ == '__main__':
    unittest.main()
