import { useState, useRef, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import Sidebar from './components/Sidebar';
import MessageBubble from './components/MessageBubble';
import MessageInput from './components/MessageInput';
import type { Message, SshConnection, CronTask } from './types';
import './App.css';

const WS_URL = 'ws://localhost:9000';

function generateId(): string {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
}

export default function App() {
  const [messages, setMessages] = useState<Message[]>([
    {
      id: '0',
      content: 'retana 已启动 ✨ 等待 Hermes 连接...',
      sender: 'system',
      timestamp: Date.now(),
    },
  ]);
  const [connections, setConnections] = useState<SshConnection[]>([]);
  const [activeConnectionId, setActiveConnectionId] = useState<string | null>(null);
  const [cronTasks, setCronTasks] = useState<CronTask[]>([]);
  const [activeTab, setActiveTab] = useState<'chat' | 'cron' | 'memory'>('chat');
  const [wsConnected, setWsConnected] = useState(false);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const wsRef = useRef<WebSocket | null>(null);

  // Connect to local WebSocket server (Hermes reaches this via reverse tunnel)
  useEffect(() => {
    let reconnectTimer: ReturnType<typeof setTimeout>;

    function connect() {
      const ws = new WebSocket(WS_URL);
      wsRef.current = ws;

      ws.onopen = () => {
        setWsConnected(true);
        setMessages(prev => [
          ...prev,
          {
            id: generateId(),
            content: '🟢 本地端点已就绪',
            sender: 'system',
            timestamp: Date.now(),
          },
        ]);
      };

      ws.onmessage = (event) => {
        try {
          const data = JSON.parse(event.data);
          handleServerMessage(data);
        } catch {
          // Non-JSON message — display as plain text
          setMessages(prev => [
            ...prev,
            {
              id: generateId(),
              content: event.data,
              sender: 'hermes',
              timestamp: Date.now(),
            },
          ]);
        }
      };

      ws.onclose = () => {
        setWsConnected(false);
        setMessages(prev => [
          ...prev,
          {
            id: generateId(),
            content: '🔴 本地端点断开，3秒后重连...',
            sender: 'system',
            timestamp: Date.now(),
          },
        ]);
        reconnectTimer = setTimeout(connect, 3000);
      };

      ws.onerror = () => {
        // onclose will fire after this
      };
    }

    connect();

    return () => {
      clearTimeout(reconnectTimer);
      wsRef.current?.close();
    };
  }, []);

  const handleServerMessage = useCallback((data: Record<string, unknown>) => {
    const msgType = data.type as string | undefined;

    if (msgType === 'tool_progress') {
      // Hermes is performing an operation
      const op = {
        id: generateId(),
        type: (data.tool_type as 'thinking' | 'tool_call' | 'tool_result' | 'error') || 'tool_call',
        label: (data.label as string) || 'working...',
        detail: data.detail as string | undefined,
        status: (data.status as 'running' | 'done' | 'error') || 'running',
      };

      setMessages(prev => {
        // Update or create a message with operations
        const lastMsg = prev[prev.length - 1];
        if (lastMsg && lastMsg.sender === 'hermes' && lastMsg.operations) {
          // Update existing operation
          const existingOp = lastMsg.operations.find(o => o.label === op.label);
          if (existingOp) {
            return prev.map(m =>
              m.id === lastMsg.id
                ? { ...m, operations: m.operations?.map(o => (o.label === op.label ? op : o)) }
                : m
            );
          }
          // Add new operation
          return prev.map(m =>
            m.id === lastMsg.id
              ? { ...m, operations: [...(m.operations || []), op] }
              : m
          );
        }
        // Create new operation message
        return [
          ...prev,
          {
            id: generateId(),
            content: '',
            sender: 'hermes' as const,
            timestamp: Date.now(),
            operations: [op],
          },
        ];
      });
    } else if (msgType === 'tool_call') {
      // Hermes wants retana to execute a command locally
      const taskId = (data.task_id as string) || generateId();
      const command = data.command as string;
      const label = (data.label as string) || command;

      handleServerMessage({
        type: 'tool_progress',
        label,
        tool_type: 'tool_call',
        status: 'running',
      });

      invoke<{ stdout: string; stderr: string; exit_code: number; success: boolean }>(
        'exec_local',
        { command }
      )
        .then((result) => {
          if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
            wsRef.current.send(JSON.stringify({
              type: 'tool_result',
              task_id: taskId,
              output: result.stdout || result.stderr,
              exit_code: result.exit_code,
              success: result.success,
            }));
          }
          handleServerMessage({
            type: 'tool_progress',
            label,
            tool_type: 'tool_call',
            status: result.success ? 'done' : 'error',
            detail: (result.stdout + result.stderr).slice(0, 200),
          });
        })
        .catch((err) => {
          if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
            wsRef.current.send(JSON.stringify({
              type: 'tool_result',
              task_id: taskId,
              output: String(err),
              exit_code: -1,
              success: false,
            }));
          }
          handleServerMessage({
            type: 'tool_progress',
            label,
            tool_type: 'tool_call',
            status: 'error',
            detail: String(err),
          });
        });
    } else {
      if (data.sender === 'user') {
        return;
      }
      const msg: Message = {
        id: generateId(),
        content: (data.content as string) || '',
        sender: (data.sender as 'user' | 'hermes' | 'system') || 'hermes',
        timestamp: Date.now(),
      };
      setMessages(prev => [...prev, msg]);
    }
  }, []);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  const handleSend = useCallback((text: string) => {
    // Show user message immediately
    const userMsg: Message = {
      id: generateId(),
      content: text,
      sender: 'user',
      timestamp: Date.now(),
    };
    setMessages(prev => [...prev, userMsg]);

    // Send via WebSocket to local server → Hermes through tunnel
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify({
        type: 'chat',
        content: text,
        sender: 'user',
      }));
    } else {
      setMessages(prev => [
        ...prev,
        {
          id: generateId(),
          content: '⚠ 本地端点未连接。请确认 Hermes 通过反向隧道已接入。',
          sender: 'system',
          timestamp: Date.now(),
        },
      ]);
    }
  }, []);

  const handleConnect = useCallback((id: string) => {
    setConnections(prev =>
      prev.map(c =>
        c.id === id ? { ...c, status: 'connecting' } : c
      )
    );
    setActiveConnectionId(id);

    // TODO: wire to actual SSH connect via Tauri invoke
    setTimeout(() => {
      setConnections(prev =>
        prev.map(c =>
          c.id === id ? { ...c, status: 'connected' } : c
        )
      );

      setMessages(prev => [
        ...prev,
        {
          id: generateId(),
          content: `SSH 隧道已建立 — 远程端口已转发到本地`,
          sender: 'system',
          timestamp: Date.now(),
        },
      ]);
    }, 1500);
  }, []);

  const handleDisconnect = useCallback((id: string) => {
    setConnections(prev =>
      prev.map(c =>
        c.id === id ? { ...c, status: 'disconnected' } : c
      )
    );

    setMessages(prev => [
      ...prev,
      {
        id: generateId(),
        content: 'SSH 隧道已断开',
        sender: 'system',
        timestamp: Date.now(),
      },
    ]);
  }, []);

  const handleAddConnection = useCallback(() => {
    const newConn: SshConnection = {
      id: generateId(),
      name: `hermes-${connections.length + 1}`,
      host: 'your-server.com',
      port: 22,
      username: 'user',
      status: 'disconnected',
    };
    setConnections(prev => [...prev, newConn]);
  }, [connections.length]);

  const handleToggleCron = useCallback((id: string, enabled: boolean) => {
    setCronTasks(prev =>
      prev.map(t => (t.id === id ? { ...t, enabled } : t))
    );
  }, []);

  return (
    <div className="app-layout">
      <Sidebar
        connections={connections}
        activeConnectionId={activeConnectionId}
        onSelect={setActiveConnectionId}
        onConnect={handleConnect}
        onDisconnect={handleDisconnect}
        onAdd={handleAddConnection}
        cronTasks={cronTasks}
        onToggleCron={handleToggleCron}
        activeTab={activeTab}
        onTabChange={setActiveTab}
      />
      <div className="main-chat">
        <div className="chat-header">
          <div className="connection-status">
            <span className={`status-dot ${wsConnected ? 'status-connected' : 'status-disconnected'}`} />
            <span>
              {wsConnected
                ? '本地端点已就绪 — 等待 Hermes 接入'
                : '本地端点未连接'}
            </span>
          </div>
          <span className="ssh-badge">WS</span>
        </div>
        <div className="messages-container">
          {messages.map(msg => (
            <MessageBubble key={msg.id} message={msg} />
          ))}
          <div ref={messagesEndRef} />
        </div>
        <MessageInput
          onSend={handleSend}
          placeholder={
            wsConnected
              ? '输入消息... (Enter 发送)'
              : '等待本地端点就绪...'
          }
        />
      </div>
    </div>
  );
}
