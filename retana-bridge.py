#!/usr/bin/env python3
"""
retana-bridge — Hermes ↔ retana WebSocket Server

运行在 Hermes 服务器上。retana 客户端直接通过 WebSocket 连接。
不需要 SSH 反向隧道。

协议 (WebSocket JSON):

retana → bridge:
  {"type": "chat", "content": "...", "sender": "user"}
  {"type": "tool_result", "task_id": "...", "output": "...", "exit_code": 0}

bridge → retana:
  {"type": "chat", "content": "...", "sender": "hermes"}
  {"type": "tool_progress", "label": "...", "tool_type": "tool_call", "status": "running"}
  {"type": "tool_call", "task_id": "...", "command": "...", "label": "..."}

用法:
  python3 retana-bridge.py [host] [port]
  默认: 0.0.0.0:9001
"""

import asyncio
import json
import os
import sys
import traceback

import aiohttp
from aiohttp import web

# ─── 配置 ───

HOST = sys.argv[1] if len(sys.argv) > 1 else "0.0.0.0"
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 9001
HERMES_API_URL = os.environ.get("HERMES_API_URL", "http://localhost:8642/v1")

# 从 .env 读 key
_api_key = None
env_path = os.path.expanduser("~/.hermes/.env")
try:
    with open(env_path) as f:
        for line in f:
            if line.startswith("API_SERVER_KEY="):
                _api_key = line.split("=", 1)[1].strip()
                break
except Exception:
    pass
HERMES_API_KEY= _api_key or "change-me-local-dev"

# ─── WebSocket 管理 ───

class BridgeServer:
    def __init__(self):
        self.clients: set[web.WebSocketResponse] = set()

    async def broadcast(self, msg: dict):
        """发送 JSON 给所有连接的 retana 客户端"""
        dead = set()
        for ws in self.clients:
            try:
                await ws.send_json(msg)
            except Exception:
                dead.add(ws)
        self.clients -= dead
        if dead:
            print(f"[bridge] 清理了 {len(dead)} 个断开的客户端", file=sys.stderr)

    async def handle_chat(self, content: str):
        """用户消息 → Hermes API 流式 → 广播回复"""
        headers = {
            "Authorization": f"Bearer {HERMES_API_KEY}",
            "Content-Type": "application/json",
        }
        body = {
            "model": "hermes-agent",
            "messages": [{"role": "user", "content": content}],
            "stream": True,
        }

        try:
            async with aiohttp.ClientSession() as session:
                async with session.post(
                    f"{HERMES_API_URL}/chat/completions",
                    headers=headers,
                    json=body,
                ) as resp:
                    if resp.status != 200:
                        text = await resp.text()
                        await self.broadcast({
                            "type": "chat",
                            "content": f"❌ API 错误 {resp.status}",
                            "sender": "system",
                        })
                        return

                    full_content = ""
                    async for line in resp.content:
                        line = line.decode("utf-8").strip()
                        if not line.startswith("data: "):
                            continue
                        data_str = line[6:]
                        if data_str == "[DONE]":
                            break

                        try:
                            data = json.loads(data_str)
                        except json.JSONDecodeError:
                            continue

                        # Hermes tool progress 事件
                        if data.get("type") == "hermes.tool.progress":
                            label = data.get("label", "working...")
                            await self.broadcast({
                                "type": "tool_progress",
                                "label": label,
                                "tool_type": "tool_call",
                                "status": "running",
                            })
                            continue

                        # 标准 OpenAI chunk
                        choices = data.get("choices", [])
                        if choices:
                            delta = choices[0].get("delta", {})
                            chunk_content = delta.get("content", "")
                            if chunk_content:
                                full_content += chunk_content

                    if full_content.strip():
                        await self.broadcast({
                            "type": "chat",
                            "content": full_content.strip(),
                            "sender": "hermes",
                        })

        except Exception as e:
            traceback.print_exc()
            await self.broadcast({
                "type": "chat",
                "content": f"❌ 桥接错误: {e}",
                "sender": "system",
            })

    async def ws_handler(self, request: web.Request) -> web.WebSocketResponse:
        ws = web.WebSocketResponse()
        await ws.prepare(request)
        self.clients.add(ws)
        peer = request.remote
        print(f"[bridge] ✅ retana 已连接: {peer}", file=sys.stderr)

        # 通知所有客户端有新连接
        await self.broadcast({
            "type": "chat",
            "content": f"🟢 retana 已接入 Hermes",
            "sender": "system",
        })

        try:
            async for msg in ws:
                if msg.type == aiohttp.WSMsgType.TEXT:
                    try:
                        data = json.loads(msg.data)
                    except json.JSONDecodeError:
                        continue

                    msg_type = data.get("type", "")

                    if msg_type == "chat" and data.get("sender") == "user":
                        asyncio.create_task(self.handle_chat(data.get("content", "")))

                    elif msg_type == "tool_result":
                        # retana 汇报本机命令执行结果
                        print(f"[bridge] tool_result: {data.get('task_id','?')} ok={data.get('success')}", file=sys.stderr)

                    else:
                        pass  # 忽略

                elif msg.type == aiohttp.WSMsgType.ERROR:
                    print(f"[bridge] WS error: {ws.exception()}", file=sys.stderr)

        except Exception as e:
            print(f"[bridge] WS 异常: {e}", file=sys.stderr)
        finally:
            self.clients.discard(ws)
            print(f"[bridge] retana 已断开: {peer}", file=sys.stderr)
            await self.broadcast({
                "type": "chat",
                "content": "🔴 retana 已断开",
                "sender": "system",
            })

        return ws


async def main():
    server = BridgeServer()
    app = web.Application()
    app.router.add_get("/ws", server.ws_handler)

    # 健康检查
    async def health(_request):
        return web.json_response({"status": "ok", "clients": len(server.clients)})
    app.router.add_get("/health", health)

    print(f"═" * 50, file=sys.stderr)
    print(f"retana-bridge WS Server", file=sys.stderr)
    print(f"  监听: ws://{HOST}:{PORT}/ws", file=sys.stderr)
    print(f"  API:  {HERMES_API_URL}", file=sys.stderr)
    print(f"═" * 50, file=sys.stderr)

    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, HOST, PORT)
    await site.start()
    print(f"[bridge] ✅ 服务已启动", file=sys.stderr)

    # 永远运行
    await asyncio.Event().wait()


if __name__ == "__main__":
    asyncio.run(main())
