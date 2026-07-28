import type { Message, HermesOperation } from '../types';

interface Props {
  message: Message;
}

function OperationBadge({ op }: { op: HermesOperation }) {
  const statusIcon = op.status === 'running' ? '◎' : op.status === 'done' ? '●' : '✕';
  const statusColor = op.status === 'running' ? '#f0a040' : op.status === 'done' ? '#40c060' : '#e04040';

  return (
    <div className={`operation-badge operation-${op.type}`}>
      <span className="op-status" style={{ color: statusColor }}>{statusIcon}</span>
      <span className="op-label">{op.label}</span>
      {op.detail && <span className="op-detail">{op.detail}</span>}
    </div>
  );
}

export default function MessageBubble({ message }: Props) {
  const isUser = message.sender === 'user';
  const isSystem = message.sender === 'system';

  if (isSystem) {
    return (
      <div className="message-system">
        <span>{message.content}</span>
      </div>
    );
  }

  return (
    <div className={`message-row ${isUser ? 'message-user' : 'message-hermes'}`}>
      {!isUser && (
        <div className="message-avatar hermes-avatar">R</div>
      )}
      <div className="message-body">
        <div className={`message-bubble ${isUser ? 'bubble-user' : 'bubble-hermes'}`}>
          <div className="message-text">{message.content}</div>
          {message.operations && message.operations.length > 0 && (
            <div className="message-operations">
              {message.operations.map(op => (
                <OperationBadge key={op.id} op={op} />
              ))}
            </div>
          )}
        </div>
        <div className="message-time">{formatTime(message.timestamp)}</div>
      </div>
      {isUser && (
        <div className="message-avatar user-avatar">U</div>
      )}
    </div>
  );
}

function formatTime(ts: number): string {
  const d = new Date(ts);
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}
