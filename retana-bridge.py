#!/usr/bin/env python3
# retana-bridge — Hermes ↔ retana WebSocket bridge (NaCl encrypted)
#
# Protocol:
#   retana → bridge: Binary = SealedBox(bridge_pk, ephemeral_pk(32) || json_message)
#   bridge → retana: Binary = Box(bridge_sk, ephemeral_pk, json_response)
#   Unencrypted Text messages also supported for compatibility.
import asyncio, json, os, re, sys, traceback, uuid
import aiohttp
from aiohttp import web
from nacl.public import PrivateKey, PublicKey, SealedBox, Box
from nacl.encoding import RawEncoder

HOST = sys.argv[1] if len(sys.argv) > 1 else "0.0.0.0"
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 9001
API_URL = "http://localhost:8642/v1"

# API key
_API_KEY = None
env_path = os.path.expanduser("~/.hermes/.env")
try:
    with open(env_path) as f:
        for line in f:
            if line.startswith("API_SERVER_KEY="):
                _API_KEY = line.split("=", 1)[1].strip()
                break
except: pass
API_KEY = _API_KEY or "change-me-local-dev"

# NaCl key
_key_path = os.path.expanduser("~/.retana/bridge_nacl.key")
try:
    with open(_key_path) as f:
        seed = bytes.fromhex(f.read().strip())
    BRIDGE_SK = PrivateKey(seed, RawEncoder)
except Exception as e:
    print(f"Error loading bridge_nacl.key: {e}", file=sys.stderr)
    BRIDGE_SK = PrivateKey.generate()
    with open(_key_path, "w") as f:
        f.write(bytes(BRIDGE_SK).hex())
    print(f"Generated new key, pubkey: {bytes(BRIDGE_SK.public_key).hex()}", file=sys.stderr)

BRIDGE_PK = BRIDGE_SK.public_key

SYS = """You are speaking through retana, a Tauri desktop app running on the user's local machine.
To run commands on the user's machine, output: [EXEC:command]
The command will be executed in the user's default shell (powershell on Windows, zsh/bash on macOS/Linux).
Examples: [EXEC:dir C:\\Users] (Windows) or [EXEC:ls -la ~/Desktop] (macOS/Linux)
Do NOT include [EXEC:...] in your final reply to the user — it is an internal mechanism. Wait for the execution result before responding."""

class B:
    def __init__(s):
        s.cs = set()       # connected WebSocket clients
        s.pe = {}          # pending exec futures: task_id → Future
        s.eph = {}         # client ephemeral keys: ws → PublicKey

    async def bc(s, m, ws=None, encrypt=True):
        """Broadcast message to all clients. If encrypt=True, use Box for each client."""
        dead = set()
        for w in s.cs:
            try:
                if encrypt and isinstance(m, dict) and w in s.eph:
                    box = Box(BRIDGE_SK, s.eph[w])
                    enc = box.encrypt(json.dumps(m).encode())
                    await w.send_bytes(enc)
                else:
                    await w.send_json(m)
            except:
                dead.add(w)
        s.cs -= dead

    async def ch(s, msgs):
        h = {"Authorization": f"Bearer {API_KEY}", "Content-Type": "application/json"}
        b = {"model": "hermes-agent", "messages": msgs, "stream": True}
        try:
            async with aiohttp.ClientSession() as sess:
                async with sess.post(API_URL + "/chat/completions", headers=h, json=b) as r:
                    if r.status != 200:
                        return f"API err {r.status}"
                    full = ""
                    async for line in r.content:
                        line = line.decode().strip()
                        if not line.startswith("data: "): continue
                        ds = line[6:]
                        if ds == "[DONE]": break
                        try: d = json.loads(ds)
                        except: continue
                        if d.get("type") == "hermes.tool.progress":
                            await s.bc({"type": "tp", "label": d.get("label", ""), "status": "running"})
                            continue
                        for ch in d.get("choices", []):
                            c = ch.get("delta", {}).get("content", "")
                            if c: full += c
                    return full.strip()
        except Exception as e:
            traceback.print_exc()
            return f"Err: {e}"

    async def hc(s, content, ws):
        msgs = [{"role": "system", "content": SYS}, {"role": "user", "content": content}]
        reply = await s.ch(msgs)
        msgs.append({"role": "assistant", "content": reply})
        execs = re.findall(r'\[EXEC:(.+?)\]', reply)
        if execs:
            for cmd in execs:
                cmd = cmd.strip()
                tid = str(uuid.uuid4())[:8]
                await s.bc({"type": "tool_call", "task_id": tid, "command": cmd, "label": cmd[:60]})
                fut = asyncio.get_event_loop().create_future()
                s.pe[tid] = fut
                try:
                    result = await asyncio.wait_for(fut, timeout=30)
                except asyncio.TimeoutError:
                    result = {"output": "timeout", "exit_code": -1}
                s.pe.pop(tid, None)
                ok = "OK" if result.get("success") else f"exit={result.get('exit_code', -1)}"
                msgs.append({"role": "user", "content": f"[EXEC:{cmd}] {ok}:\n{result.get('output', '')[:4000]}"})
            reply2 = await s.ch(msgs)
            if reply2: reply = reply2
        if reply:
            await s.bc({"type": "chat", "content": reply, "sender": "hermes"})

    async def wh(s, req):
        ws = web.WebSocketResponse()
        await ws.prepare(req)
        s.cs.add(ws)
        # Send current public key first (unencrypted, so retana can seal)
        await ws.send_json({"type": "key", "pubkey": bytes(BRIDGE_PK).hex()})
        await s.bc({"type": "chat", "content": "retana connected", "sender": "system"}, encrypt=False)

        try:
            async for msg in ws:
                if msg.type == aiohttp.WSMsgType.BINARY:
                    s._handle_binary(msg.data, ws)
                elif msg.type == aiohttp.WSMsgType.TEXT:
                    try:
                        d = json.loads(msg.data)
                    except:
                        continue
                    t = d.get("type", "")
                    if t == "chat" and d.get("sender") == "user":
                        asyncio.create_task(s.hc(d.get("content", ""), ws))
                    elif t == "tool_result":
                        tid = d.get("task_id", "")
                        if tid in s.pe:
                            s.pe[tid].set_result(d)
        except:
            pass
        finally:
            s.cs.discard(ws)
            s.eph.pop(ws, None)
        return ws

    def _handle_binary(s, data, ws):
        """Decrypt SealedBox → extract ephemeral key → process message."""
        try:
            sealed = SealedBox(BRIDGE_SK)
            plain = sealed.decrypt(data)

            # Extract ephemeral public key (first 32 bytes)
            eph_pk = PublicKey(plain[:32], RawEncoder)
            s.eph[ws] = eph_pk

            # Parse and handle the message
            msg_text = plain[32:].decode("utf-8")
            d = json.loads(msg_text)
            t = d.get("type", "")
            if t == "chat" and d.get("sender") == "user":
                asyncio.create_task(s.hc(d.get("content", ""), ws))
            elif t == "tool_result":
                tid = d.get("task_id", "")
                if tid in s.pe:
                    s.pe[tid].set_result(d)
        except Exception:
            traceback.print_exc()

async def main():
    srv = B()
    app = web.Application()
    app.router.add_get("/ws", srv.wh)
    app.router.add_get("/health", lambda r: web.json_response({"status": "ok"}))
    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, HOST, PORT)
    await site.start()
    print(f"bridge ws://{HOST}:{PORT}/ws (NaCl encrypted)", file=sys.stderr)
    print(f"pubkey: {bytes(BRIDGE_PK).hex()}", file=sys.stderr)
    await asyncio.Event().wait()

asyncio.run(main())
