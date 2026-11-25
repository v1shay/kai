#!/usr/bin/python3
"""Turn-scoped MCP transport. Authority and execution stay in Kai, never here."""
import json
import os
import socket
import sys

LIMIT = 2 * 1024 * 1024


def config():
    path = os.environ.get('KAI_WORKSPACE_CONFIG') if '--environment' in sys.argv else (sys.argv[1] if len(sys.argv) == 2 else None)
    if not path:
        return None
    with open(path, 'rb') as source:
        return json.loads(source.read(LIMIT + 1))


def dispatch(method, params, route):
    if method == 'initialize':
        return {'protocolVersion': '2024-11-05', 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'Kai Workspace', 'version': '1'}, 'instructions': route.get('instructions', '') if route else ''}
    if method == 'ping':
        return {}
    if method == 'tools/list':
        return {'tools': route['tools'] if route else []}
    if method != 'tools/call':
        raise ValueError('Unknown method')
    if not route or params.get('name') not in {t['name'] for t in route['tools']}:
        raise ValueError('Tool is not available in this invocation')
    if not isinstance(params.get('arguments', {}), dict):
        raise ValueError('Tool arguments must be an object')
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(95)
        connection.connect(route['socket'])
        message = {'token': route['token'], 'tool': params['name'], 'arguments': params.get('arguments', {})}
        payload = json.dumps(message, separators=(',', ':')).encode() + b'\n'
        if len(payload) > LIMIT:
            raise ValueError('Tool request is too large')
        connection.sendall(payload)
        with connection.makefile('rb') as source:
            line = source.readline(LIMIT + 1)
        if len(line) > LIMIT or not line.endswith(b'\n'):
            raise ValueError('Invalid workspace reply')
        value = json.loads(line)
    return {'content': [{'type': 'text', 'text': json.dumps(value, ensure_ascii=False)}], 'isError': isinstance(value, dict) and 'error' in value}


def main():
    try:
        route = config()
    except (OSError, ValueError):
        print('Kai workspace invocation is unavailable', file=sys.stderr)
        return 1
    while True:
        line = sys.stdin.buffer.readline(LIMIT + 1)
        if not line:
            return
        if len(line) > LIMIT:
            return
        request = {}
        try:
            request = json.loads(line)
            if not isinstance(request, dict):
                raise ValueError('Request must be an object')
            if 'id' not in request:
                continue
            result = dispatch(request.get('method'), request.get('params') or {}, route)
            response = {'jsonrpc': '2.0', 'id': request['id'], 'result': result}
        except (OSError, ValueError, KeyError, TypeError):
            response = {'jsonrpc': '2.0', 'id': request.get('id') if isinstance(request, dict) else None, 'error': {'code': -32603, 'message': 'Workspace tool unavailable or request invalid'}}
        print(json.dumps(response, ensure_ascii=False), flush=True)


if __name__ == '__main__':
    sys.exit(main())
