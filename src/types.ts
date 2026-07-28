// Type definitions for retana

export interface Message {
  id: string;
  content: string;
  sender: 'user' | 'hermes' | 'system';
  timestamp: number;
  /** Tool calls or operations Hermes is performing */
  operations?: HermesOperation[];
}

export interface HermesOperation {
  id: string;
  type: 'thinking' | 'tool_call' | 'tool_result' | 'error';
  label: string;
  detail?: string;
  status: 'running' | 'done' | 'error';
}

export interface SshConnection {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  status: 'connected' | 'disconnected' | 'connecting' | 'error';
  lastError?: string;
}

export interface CronTask {
  id: string;
  name: string;
  command: string;
  schedule: string;
  enabled: boolean;
  last_run?: string;
}

export interface MemoryEntry {
  key: string;
  value: string;
  category: string;
}

export interface SystemInfo {
  os: string;
  arch: string;
  hostname: string;
}
