import type { SshConnection, CronTask } from '../types';

interface Props {
  connections: SshConnection[];
  activeConnectionId: string | null;
  onSelect: (id: string) => void;
  onConnect: (id: string) => void;
  onDisconnect: (id: string) => void;
  onAdd: () => void;
  cronTasks: CronTask[];
  onToggleCron: (id: string, enabled: boolean) => void;
  activeTab: 'chat' | 'cron' | 'memory';
  onTabChange: (tab: 'chat' | 'cron' | 'memory') => void;
}

export default function Sidebar({
  connections,
  activeConnectionId,
  onSelect,
  onConnect,
  onDisconnect,
  onAdd,
  cronTasks,
  onToggleCron,
  activeTab,
  onTabChange,
}: Props) {
  return (
    <div className="sidebar">
      <div className="sidebar-header">
        <h2>retana</h2>
      </div>

      <div className="sidebar-tabs">
        {(['chat', 'cron', 'memory'] as const).map(tab => (
          <button
            key={tab}
            className={`sidebar-tab ${activeTab === tab ? 'active' : ''}`}
            onClick={() => onTabChange(tab)}
          >
            {tab === 'chat' ? '💬' : tab === 'cron' ? '⏱' : '🧠'}
            <span>{tab === 'chat' ? 'Chat' : tab === 'cron' ? 'Cron' : 'Memory'}</span>
          </button>
        ))}
      </div>

      {activeTab === 'chat' && (
        <div className="sidebar-connections">
          <div className="section-header">
            <span>Connections</span>
            <button className="add-btn" onClick={onAdd}>+</button>
          </div>
          {connections.map(conn => (
            <div
              key={conn.id}
              className={`connection-item ${activeConnectionId === conn.id ? 'active' : ''}`}
              onClick={() => onSelect(conn.id)}
            >
              <span className={`status-dot status-${conn.status}`} />
              <div className="connection-info">
                <div className="connection-name">{conn.name}</div>
                <div className="connection-host">{conn.host}:{conn.port}</div>
              </div>
              {conn.status === 'disconnected' || conn.status === 'error' ? (
                <button
                  className="conn-action-btn"
                  onClick={e => { e.stopPropagation(); onConnect(conn.id); }}
                >
                  ▶
                </button>
              ) : (
                <button
                  className="conn-action-btn disconnect"
                  onClick={e => { e.stopPropagation(); onDisconnect(conn.id); }}
                >
                  ■
                </button>
              )}
            </div>
          ))}
        </div>
      )}

      {activeTab === 'cron' && (
        <div className="sidebar-connections">
          <div className="section-header">
            <span>Cron Tasks</span>
          </div>
          {cronTasks.map(task => (
            <div key={task.id} className="connection-item">
              <span className={`status-dot ${task.enabled ? 'status-connected' : 'status-disconnected'}`} />
              <div className="connection-info">
                <div className="connection-name">{task.name}</div>
                <div className="connection-host">{task.schedule}</div>
              </div>
              <button
                className="conn-action-btn"
                onClick={() => onToggleCron(task.id, !task.enabled)}
              >
                {task.enabled ? '⏸' : '▶'}
              </button>
            </div>
          ))}
        </div>
      )}

      {activeTab === 'memory' && (
        <div className="sidebar-connections">
          <div className="section-header">
            <span>Memory</span>
          </div>
          <div className="memory-hint">local machine context</div>
        </div>
      )}
    </div>
  );
}
