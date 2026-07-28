import { useState, useRef, useEffect, useCallback } from 'react';
import Sidebar from './components/Sidebar';
import MessageBubble from './components/MessageBubble';
import MessageInput from './components/MessageInput';
import type { Message, SshConnection, CronTask } from './types';
import './App.css';

// Mock connections for now
const DEFAULT_CONNECTIONS: SshConnection[] = [
  {
    id: '1',
    name: 'home-server',
    host: '192.168.1.100',
    port: 22,
    username: 'ubuntu',
    status: 'disconnected',
  },
];

const DEFAULT_CRON: CronTask[] = [
  {
    id: 'c1',
    name: 'Health Check',
    command: 'curl -s http://localhost:8080/health',
    schedule: '5m',
    enabled: false,
  },
];

function generateId(): string {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
}

export default function App() {
  const [messages, setMessages] = useState<Message[]>([
    {
      id: '0',
      content: 'retana 已启动 ✨ 连接一个 Hermes 实例开始聊天吧~',
      sender: 'system',
      timestamp: Date.now(),
    },
  ]);
  const [connections, setConnections] = useState<SshConnection[]>(DEFAULT_CONNECTIONS);
  const [activeConnectionId, setActiveConnectionId] = useState<string | null>(null);
  const [cronTasks, setCronTasks] = useState<CronTask[]>(DEFAULT_CRON);
  const [activeTab, setActiveTab] = useState<'chat' | 'cron' | 'memory'>('chat');
  const [isConnected, setIsConnected] = useState(false);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  const handleSend = useCallback((text: string) => {
    const userMsg: Message = {
      id: generateId(),
      content: text,
      sender: 'user',
      timestamp: Date.now(),
    };
    setMessages(prev => [...prev, userMsg]);

    // Simulate Hermes response with operations
    if (isConnected) {
      const thinkingOp = {
        id: generateId(),
        type: 'thinking' as const,
        label: '思考中…',
        status: 'running' as const,
      };

      const thinkingMsg: Message = {
        id: generateId(),
        content: '',
        sender: 'hermes',
        timestamp: Date.now(),
        operations: [thinkingOp],
      };
      setMessages(prev => [...prev, thinkingMsg]);

      // Simulate response after delay
      setTimeout(() => {
        setMessages(prev =>
          prev.map(m =>
            m.id === thinkingMsg.id
              ? {
                  ...m,
                  content: `收到你的消息: "${text}" — 这是来自 Hermes 的模拟回复~ (◕‿◕)`,
                  operations: m.operations?.map(op => ({ ...op, status: 'done' as const })),
                }
              : m
          )
        );
      }, 1500);
    } else {
      // Not connected
      setTimeout(() => {
        const sysMsg: Message = {
          id: generateId(),
          content: '⚠ 尚未连接到 Hermes 实例。请先在侧边栏连接 SSH。',
          sender: 'system',
          timestamp: Date.now(),
        };
        setMessages(prev => [...prev, sysMsg]);
      }, 300);
    }
  }, [isConnected]);

  const handleConnect = useCallback((id: string) => {
    setConnections(prev =>
      prev.map(c =>
        c.id === id ? { ...c, status: 'connecting' } : c
      )
    );
    setActiveConnectionId(id);

    setTimeout(() => {
      setConnections(prev =>
        prev.map(c =>
          c.id === id ? { ...c, status: 'connected' } : c
        )
      );
      setIsConnected(true);
      setActiveConnectionId(id);

      const sysMsg: Message = {
        id: generateId(),
        content: `已连接到 ${connections.find(c => c.id === id)?.name || 'Hermes'} ✅`,
        sender: 'system',
        timestamp: Date.now(),
      };
      setMessages(prev => [...prev, sysMsg]);
    }, 1000);
  }, [connections]);

  const handleDisconnect = useCallback((id: string) => {
    setConnections(prev =>
      prev.map(c =>
        c.id === id ? { ...c, status: 'disconnected' } : c
      )
    );
    setIsConnected(false);

    const sysMsg: Message = {
      id: generateId(),
      content: '已断开连接',
      sender: 'system',
      timestamp: Date.now(),
    };
    setMessages(prev => [...prev, sysMsg]);
  }, []);

  const handleAddConnection = useCallback(() => {
    const newConn: SshConnection = {
      id: generateId(),
      name: `server-${connections.length + 1}`,
      host: 'localhost',
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
            <span className={`status-dot ${isConnected ? 'status-connected' : 'status-disconnected'}`} />
            <span>
              {isConnected
                ? `已连接 — ${connections.find(c => c.id === activeConnectionId)?.name || 'Hermes'}`
                : '未连接'}
            </span>
          </div>
          {isConnected && (
            <span className="ssh-badge">SSH</span>
          )}
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
            isConnected
              ? '输入消息... (Enter 发送)'
              : '请先连接 SSH...'
          }
        />
      </div>
    </div>
  );
}
