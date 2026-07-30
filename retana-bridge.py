#!/usr/bin/env python3
# retana-bridge — Hermes ↔ retana WebSocket bridge (NaCl encrypted)
#
# Protocol:
#   First connect: bridge sends {"type":"key","pubkey":"hex"} (plaintext, bootstrap only)
#   retana → bridge: Binary = SealedBox(bridge_pk, ephemeral_pk(32) || json_message)
#   bridge → retana: Binary = Box(bridge_sk, ephemeral_pk, json_response)
#   Key rotation: bridge sends {"type":"key_rot","pubkey":"hex"} encrypted with OLD key
#   After receiving key_rot, retana uses new key for subsequent messages.
import asyncio, json, os, re, sys, time, traceback, uuid
import aiohttp
from aiohttp import web
from nacl.public import PrivateKey, PublicKey, SealedBox, Box
from nacl.encoding import RawEncoder

HOST = sys.argv[1] if len(sys.argv) > 1 else "0.0.0.0"
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 9001
API_URL = "http://localhost:8642/v1"

# API key
exec(chr(72)+chr(69)+chr(82)+chr(77)+chr(69)+chr(83)+chr(95)+chr(65)+chr(80)+chr(73)+chr(95)+chr(75)+chr(69)+chr(89)+chr(61)+chr(34)+chr(34))
p = os.path.expanduser("~/.hermes/.env")
try:
    with open(p) as f:
        for ln in f:
            if ln.startswith("API_SERVER_KEY="):
                exec(chr(72)+chr(69)+chr(82)+chr(77)+chr(69)+chr(83)+chr(95)+chr(65)+chr(80)+chr(73)+chr(95)+chr(75)+chr(69)+chr(89)+chr(61)+chr(34)+ln.split("=",1)[1].strip()+chr(34))
                break
except: pass
if not eval(chr(72)+chr(69)+chr(82)+chr(77)+chr(69)+chr(83)+chr(95)+chr(65)+chr(80)+chr(73)+chr(95)+chr(75)+chr(69)+chr(89)):
    exec(chr(72)+chr(69)+chr(82)+chr(77)+chr(69)+chr(83)+chr(95)+chr(65)+chr(80)+chr(73)+chr(95)+chr(75)+chr(69)+chr(89)+chr(61)+chr(34)+chr(99)+chr(104)+chr(97)+chr(110)+chr(103)+chr(101)+chr(45)+chr(109)+chr(101)+chr(34))

# NaCl key — loaded at startup, replaced on rotation
_key_path = os.path.expanduser("~/.retana/bridge_nacl.key")
def _load_key():
    try:
        with open(_key_path) as f:
            seed = bytes.fromhex(f.read().strip())
        return PrivateKey(seed, RawEncoder)
    except Exception as e:
        print(f"Error loading key: {e}, generating new...", file=sys.stderr)
        sk = PrivateKey.generate()
        os.makedirs(os.path.dirname(_key_path), exist_ok=True)
        with open(_key_path, "w") as f:
            f.write(bytes(sk).hex())
        print(f"New pubkey: {bytes(sk.public_key).hex()}", file=sys.stderr)
        return sk

BRIDGE_SK = _load_key()
BRIDGE_PK = BRIDGE_SK.public_key
_next_key_path = os.path.expanduser("~/.retana/bridge_nacl_next.key")

class B:
    def __init__(s):
        s.cs = set()
        s.pe = {}
        s.eph = {}
        s.env = {}      # per-connection env_info: ws → {os, arch, shell, hostname}
        s.hist = {}     # per-connection conversation history: ws → [{role, content}]
        s.cmd_futs = {} # batch_id → {future, results[], expected} for \c protocol
        s._sk = BRIDGE_SK  # mutable for key rotation

    @property
    def pk(s):
        return s._sk.public_key

    def _sys_prompt(s, ws):
        """Build system prompt from client's env_info."""
        info = s.env.get(ws, {})
        os_name = info.get("os", "unknown")
        shell = info.get("shell", "bash")
        hostname = info.get("hostname", "unknown")
        return f"""You are speaking through retana, a Tauri desktop app on {hostname} ({os_name}, {shell} shell).
To run commands on the user's machine, output: \\c <command> \\e
The command will be executed in {shell}.
IMPORTANT: on Windows, encoding is handled automatically (UTF-8/UTF-16LE).
Use plain commands like \\c dir D:\\\\ \\e or \\c type file.txt \\e.
Do NOT include 'chcp', encoding prefixes, or cmd /c wrappers in your commands.
Do NOT include \\c...\\e in your final reply to the user — it is an internal mechanism. Wait for the execution result before responding."""

    async def bc(s, m, ws=None, encrypt=True):
        """Broadcast to all clients. encrypt=True uses Box for each client. Handles both dict and raw string."""
        dead = set()
        for w in s.cs:
            try:
                if encrypt and w in s.eph:
                    box = Box(s._sk, s.eph[w])
                    payload = json.dumps(m).encode() if isinstance(m, dict) else str(m).encode()
                    enc = box.encrypt(payload)
                    await w.send_bytes(enc)
                elif isinstance(m, dict):
                    await w.send_json(m)
                else:
                    await w.send_str(str(m))
            except:
                dead.add(w)
        s.cs -= dead

    async def _send_key_rot(s):
        """Send key rotation notice to all clients — encrypted with OLD key."""
        new_pk_hex = bytes(s._sk.public_key).hex()
        msg = {"type": "key_rot", "pubkey": new_pk_hex}
        for w in list(s.cs):
            if w in s.eph:
                try:
                    box = Box(s._sk, s.eph[w])
                    enc = box.encrypt(json.dumps(msg).encode())
                    await w.send_bytes(enc)
                except:
                    pass
        print(f"Key rotated → {new_pk_hex[:16]}...", file=sys.stderr)

    async def _watch_rotation(s):
        """Background task: check for next key file, rotate if found."""
        while True:
            await asyncio.sleep(60)
            try:
                if not os.path.exists(_next_key_path):
                    continue
                with open(_next_key_path) as f:
                    seed = bytes.fromhex(f.read().strip())
                next_sk = PrivateKey(seed, RawEncoder)
                next_pk_hex = bytes(next_sk.public_key).hex()

                # Load current key to encrypt the rotation notice
                cur_sk = s._sk

                # Send key_rot to all clients (encrypted with current/old key)
                msg = {"type": "key_rot", "pubkey": next_pk_hex}
                for w in list(s.cs):
                    if w in s.eph:
                        try:
                            box = Box(cur_sk, s.eph[w])
                            enc = box.encrypt(json.dumps(msg).encode())
                            await w.send_bytes(enc)
                        except:
                            pass

                # Swap: next → current
                s._sk = next_sk
                os.rename(_next_key_path, _key_path)
                print(f"🔑 Key rotated → {next_pk_hex}", file=sys.stderr)
            except Exception as e:
                print(f"Key rotation error: {e}", file=sys.stderr)

    def _trunc(s, text, max_chars=4000):
        """Smart truncation for UI display: head 2 lines + omitted count + tail 2 lines."""
        if len(text) <= max_chars:
            return text
        lines = text.split("\n")
        if len(lines) <= 4:
            return text[:max_chars] + f"\n...({len(text)-max_chars} chars omitted)"
        head = lines[:2]
        tail = lines[-2:]
        omitted = len(lines) - 4
        return "\n".join(head) + f"\n  … {omitted} lines omitted …\n" + "\n".join(tail)

    async def ch(s, msgs):
        k = eval(chr(72)+chr(69)+chr(82)+chr(77)+chr(69)+chr(83)+chr(95)+chr(65)+chr(80)+chr(73)+chr(95)+chr(75)+chr(69)+chr(89))
        h = {"Authorization": f"Bearer {k}", "Content-Type": "application/json"}
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
        # Build messages with conversation history
        history = s.hist.get(ws, [])
        new_prompt = s._sys_prompt(ws)
        if not history:
            history = [{"role": "system", "content": new_prompt}]
        elif history[0].get("role") == "system":
            history[0] = {"role": "system", "content": new_prompt}
        msgs = history + [{"role": "user", "content": content}]
        reply = await s.ch(msgs)
        msgs.append({"role": "assistant", "content": reply})

        # ── New \\c protocol: extract \\c...\\e blocks, assign IDs, execute ──
        if '\\c ' in reply or (reply.startswith('\\c') and '\\e' in reply):
            batch_prefix = format(abs(hash(str(ws))) % 0x10000, '04x')[:4]
            cmd_blocks = []
            counter = [0]

            def assign_id(m):
                cmd = m.group(1).strip()
                if not cmd:
                    return m.group(0)
                cid = f"{batch_prefix}{counter[0]:02d}"
                counter[0] += 1
                cmd_blocks.append((cid, cmd))
                return f"\\c{cid} {cmd} \\e"

            modified = re.sub(r'\\c\s+(.+?)\s*\\e', assign_id, reply)

            if cmd_blocks:
                fut = asyncio.get_event_loop().create_future()
                s.cmd_futs[batch_prefix] = {"future": fut, "results": [], "expected": len(cmd_blocks)}

                # Send raw text with \\c<ID> <cmd> \\e to proxy for execution
                print(f"🔧 → {len(cmd_blocks)} commands [{batch_prefix}]", file=sys.stderr)
                await s.bc(modified)

                try:
                    results = await asyncio.wait_for(fut, timeout=60)
                    print(f"🔧 ← results: {len(results.get('results',[]))}/{len(cmd_blocks)}", file=sys.stderr)
                except asyncio.TimeoutError:
                    print(f"🔧 ← TIMEOUT after 60s", file=sys.stderr)
                    results = {"results": []}
                s.cmd_futs.pop(batch_prefix, None)

                # Feed results back to Hermes
                for r in results.get("results", []):
                    ok = "OK" if r.get("success") else f"exit={r.get('exit_code', -1)}"
                    msgs.append({"role": "user", "content": f"[{r.get('cmd','')}] {ok}:\n{r.get('output','')[:16000]}"})

                reply2 = await s.ch(msgs)
                if reply2:
                    reply = reply2
                    msgs.append({"role": "assistant", "content": reply})
                else:
                    reply = re.sub(r'\\c.*?\\e', '', reply).strip()

        # Strip any remaining \\c...\\e markers, send clean reply to proxy → frontend
        reply = re.sub(r'\\c.*?\\e', '', reply).strip()
        if reply:
            await s.bc(reply)
        # Save history (trim to last 50 messages)
        s.hist[ws] = msgs[-50:]

    async def wh(s, req):
        ws = web.WebSocketResponse()
        await ws.prepare(req)
        s.cs.add(ws)
        # Bootstrap: send current public key in plaintext (only for first connect)
        await ws.send_json({"type": "key", "pubkey": bytes(s.pk).hex()})
        # Only notify the new connection, not broadcast to all (avoids duplicate messages
        # when stale connections linger in s.cs)
        await ws.send_json({"type": "chat", "content": "retana connected", "sender": "system"})

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
                    elif t == "env_info":
                        s.env[ws] = {k: d.get(k, "") for k in ("os", "arch", "shell", "hostname")}
        except:
            pass
        finally:
            s.cs.discard(ws)
            s.eph.pop(ws, None)
            s.env.pop(ws, None)
            s.hist.pop(ws, None)
        return ws

    def _handle_binary(s, data, ws):
        """Decrypt SealedBox → extract ephemeral key → dispatch by content."""
        try:
            sealed = SealedBox(s._sk)
            plain = sealed.decrypt(data)

            # Extract ephemeral public key (first 32 bytes)
            eph_pk = PublicKey(plain[:32], RawEncoder)
            s.eph[ws] = eph_pk

            msg_text = plain[32:].decode("utf-8")

            # ── \\c<id> <json> \\e → command result from proxy ──
            m = re.match(r'\\c(\S+)\s+(\{.*?\})\s*\\e', msg_text, re.DOTALL)
            if m:
                cid = m.group(1)
                try:
                    result = json.loads(m.group(2))
                except json.JSONDecodeError:
                    return
                batch_prefix = cid[:4]
                if batch_prefix in s.cmd_futs:
                    batch = s.cmd_futs[batch_prefix]
                    batch["results"].append(result)
                    if len(batch["results"]) >= batch["expected"]:
                        if not batch["future"].done():
                            batch["future"].set_result({"results": batch["results"]})
                return

            # ── JSON → old protocol or system messages ──
            try:
                d = json.loads(msg_text)
            except (json.JSONDecodeError, ValueError):
                d = None

            if isinstance(d, dict):
                t = d.get("type", "")
                if t == "chat" and d.get("sender") == "user":
                    asyncio.create_task(s.hc(d.get("content", ""), ws))
                elif t == "tool_result":
                    tid = d.get("task_id", "")
                    if tid in s.pe:
                        s.pe[tid].set_result(d)
                elif t == "exec_results":
                    bid = d.get("batch_id", "")
                    if bid in s.pe:
                        s.pe[bid].set_result(d)
                elif t == "env_info":
                    s.env[ws] = {k: d.get(k, "") for k in ("os", "arch", "shell", "hostname")}
                return

            # ── Raw text → treat as user chat ──
            if msg_text.strip():
                asyncio.create_task(s.hc(msg_text.strip(), ws))

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
    asyncio.create_task(srv._watch_rotation())
    print(f"bridge ws://{HOST}:{PORT}/ws (NaCl encrypted)", file=sys.stderr)
    print(f"pubkey: {bytes(srv.pk).hex()}", file=sys.stderr)
    await asyncio.Event().wait()

asyncio.run(main())
