import { useEffect, useState } from 'react';

import { getSafetyStatus, type SafetyStatus } from '../shared/commands/safety';

const unavailableStatus: SafetyStatus = {
  phase: '阶段 0：工程与安全底座',
  globalWriteGate: 'closed',
  importGate: 'closed',
  recycleGate: 'closed',
  databaseMode: 'unavailable',
  message: '正在连接本地安全核心…',
};

export function App() {
  const [status, setStatus] = useState<SafetyStatus>(unavailableStatus);

  useEffect(() => {
    let active = true;

    void getSafetyStatus()
      .then((nextStatus) => {
        if (active) {
          setStatus(nextStatus);
        }
      })
      .catch(() => {
        if (active) {
          setStatus({
            ...unavailableStatus,
            message:
              '无法读取安全状态。照片写入功能保持关闭；请查看开发诊断日志。',
          });
        }
      });

    return () => {
      active = false;
    };
  }, []);

  return (
    <main className="safety-shell">
      <section className="safety-card" aria-labelledby="app-title">
        <p className="eyebrow">FUJI 图库</p>
        <h1 id="app-title">本地照片管理器</h1>
        <p className="phase">{status.phase}</p>
        <p className="message">{status.message}</p>

        <dl className="status-grid">
          <StatusRow label="共享照片写入" value={status.globalWriteGate} />
          <StatusRow label="导入执行" value={status.importGate} />
          <StatusRow label="回收站执行" value={status.recycleGate} />
          <StatusRow label="应用数据库" value={status.databaseMode} />
        </dl>

        <p className="scope-note">
          当前版本不会扫描照片，不会读取内存卡，也不会导入、移动或删除任何照片。
        </p>
      </section>
    </main>
  );
}

interface StatusRowProps {
  readonly label: string;
  readonly value: string;
}

function StatusRow({ label, value }: StatusRowProps) {
  const translated = value === 'closed' ? '已关闭' : translateDatabase(value);

  return (
    <div className="status-row">
      <dt>{label}</dt>
      <dd>{translated}</dd>
    </div>
  );
}

function translateDatabase(value: string): string {
  switch (value) {
    case 'read_write':
      return '读写正常';
    case 'read_only_safe':
      return '只读安全模式';
    default:
      return '尚未连接';
  }
}
