#!/usr/bin/env python3
"""
retana-bridge — Hermes ↔ retana WebSocket 桥接

运行在 Hermes 服务器上。Hermes API Server 通过 SSH 反向隧道
连接到 retana 的 WebSocket 端点，实现双向实时聊天。

协议 (WebSocket JSON):

retana → Hermes:
  {"type": "chat", "content": "...", "sender": "user"}
  {"type": "tool_result", "task_id": "...", "output": "...", "exit_code": 0}

Hermes → retana:
  {"type": "chat", "content": "...", "sender": "hermes"}
  {"type": "tool_progress", "label": "...", "tool_type": "tool_call", "status": "running"}
  {"type": "tool_call", "task_id": "...", "command": "...", "label": "执行命令"}

用法:
  HERMES_API_URL=http://localhost:8642/v1 \
  HERMES_API_KEY=change-me-local-dev \
  python3 retana-bridge.py [ws_url]

默认 ws_url = ws://localhost:9000 (通过 SSH 反向隧道)
"""

import asyncio
import json
import os
import sys
import traceback
import uuid
from typing import Optional

import aiohttp  # pip install aiohttp

# ─── 配置 ───

WS_URL = sys.argv[1] if len(sys.argv) > 1 else "ws://localhost:9000"
HERMES_API_URL = os.environ.get("HERMES_API_URL", "http://localhost:8642/v1")
HERMES_API_KEY = os.environ.get("HERMES_API_KEY", "change-me-local-dev")

# ─── 消息处理 ───

class Bridge:
    def __init__(self):
        self.ws: Optional[aiohttp.ClientWebSocketResponse] = None
        self.pending_tools: dict[str, asyncio.Future] = {}

    async def connect_ws(self):
        """连接到 retana WebSocket（通过 SSH 隧道）"""
        while True:
            try:
                print(f"[bridge] 连接 {WS_URL} ...", file=sys.stderr)
                session = aiohttp.ClientSession()
                self.ws = await session.ws_connect(WS_URL)
                print(f"[bridge] ✅ 已连接", file=sys.stderr)
                return session
            except Exception as e:
                print(f"[bridge] ❌ 连接失败: {e}，3秒后重试", file=sys.stderr)
                await asyncio.sleep(3)

    async def send_to_retana(self, msg: dict):
        """发送 JSON 消息到 retana"""
        if self.ws and not self.ws.closed:
            await self.ws.send_json(msg)
            print(f"[bridge] → retana: {json.dumps(msg, ensure_ascii=False)[:200]}", file=sys.stderr)

    async def handle_chat(self, content: str):
        """把用户消息发给 Hermes API，流式响应发回 retana"""
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
                        await self.send_to_retana({
                            "type": "chat",
                            "content": f"❌ Hermes API 错误 {resp.status}: {text[:500]}",
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

                        # 检查是否是 Hermes 自定义事件 (tool progress)
                        if data.get("type") == "hermes.tool.progress":
                            label = data.get("label", "working...")
                            # 发送 tool_progress 给 retana
                            await self.send_to_retana({
                                "type": "tool_progress",
                                "label": label,
                                "tool_type": "tool_call",
                                "status": "running",
                            })
                            continue

                        # 标准 OpenAI 流式 chunk
                        choices = data.get("choices", [])
                        if choices:
                            delta = choices[0].get("delta", {})
                            chunk_content = delta.get("content", "")
                            if chunk_content:
                                full_content += chunk_content

                    # 流结束后，发送完整回复
                    if full_content.strip():
                        await self.send_to_retana({
                            "type": "chat",
                            "content": full_content.strip(),
                            "sender": "hermes",
                        })

        except Exception as e:
            traceback.print_exc()
            await self.send_to_retana({
                "type": "chat",
                "content": f"❌ 桥接错误: {str(e)}",
                "sender": "system",
            })

    async def handle_tool_call(self, msg: dict):
        """
        Hermes 想让 retana 执行命令（通过 tool_call 消息）。
        retana 执行后发回 tool_result。
        这里只是记录 pending，实际执行由 retana 完成。
        """
        task_id = msg.get("task_id", str(uuid.uuid4())[:8])
        command = msg.get("command", "")
        label = msg.get("label", "execute")

        print(f"[bridge] 工具调用 [{task_id}]: {label} — {command[:100]}", file=sys.stderr)

        # 这个 task 等待 retana 的 tool_result
        loop = asyncio.get_event_loop()
        fut = loop.create_future()
        self.pending_tools[task_id] = fut

        # 同时通知 Hermes（如果 Hermes 在等 tool result）
        # 这里不做额外处理，retana 执行完后会发 tool_result

    async def handle_tool_result(self, msg: dict):
        """retana 汇报命令执行结果"""
        task_id = msg.get("task_id", "")
        output = msg.get("output", "")
        exit_code = msg.get("exit_code", 0)

        print(f"[bridge] 工具结果 [{task_id}]: exit={exit_code}", file=sys.stderr)

        if task_id in self.pending_tools:
            self.pending_tools[task_id].set_result(msg)

    async def run(self):
        """主循环：接收 WS 消息 → 处理"""
        session = await self.connect_ws()

        try:
            async for msg in self.ws:
                if msg.type == aiohttp.WSMsgType.TEXT:
                    try:
                        data = json.loads(msg.data)
                    except json.JSONDecodeError:
                        print(f"[bridge] 非JSON: {msg.data[:200]}", file=sys.stderr)
                        continue

                    msg_type = data.get("type", "")

                    if msg_type == "chat" and data.get("sender") == "user":
                        # 用户消息 → 发给 Hermes
                        asyncio.create_task(self.handle_chat(data.get("content", "")))

                    elif msg_type == "tool_result":
                        await self.handle_tool_result(data)

                    elif msg_type == "chat" and data.get("sender") == "hermes":
                        # Hermes 直接发的消息（不通过本桥接）→ 忽略
                        pass

                    else:
                        print(f"[bridge] 未知消息: {msg_type}", file=sys.stderr)

                elif msg.type == aiohttp.WSMsgType.CLOSED:
                    print("[bridge] WebSocket 关闭", file=sys.stderr)
                    break
                elif msg.type == aiohttp.WSMsgType.ERROR:
                    print(f"[bridge] WS 错误", file=sys.stderr)
                    break

        except Exception as e:
            traceback.print_exc()
        finally:
            await session.close()
            # 重连
            print("[bridge] 重新连接...", file=sys.stderr)
            await asyncio.sleep(3)
            await self.run()


async def main():
    bridge = Bridge()
    await bridge.run()


if __name__ == "__main__":
    print("═" * 50, file=sys.stderr)
    print("retana-bridge — Hermes ↔ retana", file=sys.stderr)
    print(f"  WS: {WS_URL}", file=sys.stderr)
    print(f"  API: {HERMES_API_URL}", file=sys.stderr)
    print("═" * 50, file=sys.stderr)
    asyncio.run(main())
