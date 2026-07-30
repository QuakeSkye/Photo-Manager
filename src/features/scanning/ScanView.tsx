import { useRef, useState, type RefObject } from 'react';

import {
  cancelLibraryScan,
  getScanStatus,
  retryLibraryScan,
  type ScanStatus,
} from '../../shared/commands/library';
import { Modal } from '../../shared/ui/Modal';
import { TopBar } from '../../shared/ui/TopBar';

interface ScanViewProps {
  readonly scan: ScanStatus;
  readonly settingsButtonRef: RefObject<HTMLButtonElement | null>;
  readonly onBack: () => void;
  readonly onEnterLibrary: () => void;
  readonly onOpenSettings: () => void;
  readonly onStatusChange: (scan: ScanStatus) => void;
}

export function ScanView({
  scan,
  settingsButtonRef,
  onBack,
  onEnterLibrary,
  onOpenSettings,
  onStatusChange,
}: ScanViewProps) {
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const cancelButtonRef = useRef<HTMLButtonElement>(null);
  const isActive = scan.phase === 'running' || scan.phase === 'cancelling';
  const isFullGate = scan.kind === 'full';

  const cancel = async () => {
    setBusy(true);
    setError(null);
    try {
      await cancelLibraryScan();
      const next = await getScanStatus();
      if (next !== null) onStatusChange(next);
      setConfirmCancel(false);
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setBusy(false);
    }
  };

  const retry = async () => {
    setBusy(true);
    setError(null);
    try {
      await retryLibraryScan();
      const next = await getScanStatus();
      if (next !== null) onStatusChange(next);
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="screen tool-screen scan-screen">
      <TopBar
        context={isFullGate ? '首次扫描图库' : '更新图库'}
        settingsButtonRef={settingsButtonRef}
        onOpenSettings={onOpenSettings}
        onBack={!isFullGate || scan.phase === 'completed' ? onBack : undefined}
      />
      <main className="tool-workspace">
        <nav className="tool-sidebar" aria-label="扫描状态">
          <p className="sidebar-title">照片文件夹</p>
          <div className="scan-step is-active">
            <span>{isActive ? '1' : '✓'}</span>
            <div>
              <strong>{isFullGate ? '全量扫描' : '增量扫描'}</strong>
              <small>{phaseLabel(scan.phase)}</small>
            </div>
          </div>
          <div
            className={
              scan.phase === 'completed' ? 'scan-step is-active' : 'scan-step'
            }
          >
            <span>2</span>
            <div>
              <strong>扫描结果</strong>
              <small>
                {scan.phase === 'completed' ? '可以继续' : '等待完成'}
              </small>
            </div>
          </div>
        </nav>
        <section className="tool-content scan-content">
          <div className="content-heading">
            <p className="section-label">
              {isFullGate ? '首次绑定或新图库' : '原图库可靠重连'}
            </p>
            <h1>{scanTitle(scan)}</h1>
            <p>{scanDescription(scan)}</p>
          </div>
          <div className={`scan-hero phase-${scan.phase}`}>
            <div className="scan-orbit" aria-hidden="true">
              <i />
              <i />
              <span>▧</span>
            </div>
            <div className="scan-root long-path">{scan.rootPath}</div>
            <div className="scan-metrics">
              <Metric label="已检查目录" value={scan.directoriesChecked} />
              <Metric label="已检查文件" value={scan.filesChecked} />
              <Metric label="发现照片文件" value={scan.discoveredAssets} />
              <Metric label="忽略" value={scan.ignoredFiles} />
              <Metric
                label="警告"
                value={scan.warningCount}
                warning={scan.warningCount > 0}
              />
            </div>
            {scan.currentRelativePath !== null && isActive && (
              <div className="current-scan-path">
                <span>正在检查</span>
                <strong>{scan.currentRelativePath}</strong>
              </div>
            )}
            {scan.errorMessage !== null && (
              <div className="inline-error" role="alert">
                <strong>扫描未完成</strong>
                <span>{scan.errorMessage}</span>
              </div>
            )}
            {error !== null && (
              <div className="inline-error" role="alert">
                <strong>操作失败</strong>
                <span>{error}</span>
              </div>
            )}
          </div>
        </section>
        <aside className="tool-summary scan-summary">
          <div>
            <p className="section-label">扫描方式</p>
            <strong>
              {isFullGate ? '完全只读的全量扫描' : '基于既有索引的增量扫描'}
            </strong>
            <p>
              {isFullGate
                ? '扫描完成前保持在此页面，不把部分索引显示成完整图库。'
                : '可立即返回图库；后台更新不会改变当前选中的照片。'}
            </p>
            <div className="safety-fact">
              <span aria-hidden="true">✓</span>
              <div>
                <strong>照片文件保持不变</strong>
                <small>不修改名称、内容或属性</small>
              </div>
            </div>
          </div>
          <div className="summary-spacer" />
          <div className="summary-actions">
            {isActive && (
              <button
                ref={cancelButtonRef}
                type="button"
                onClick={() => setConfirmCancel(true)}
                disabled={scan.phase === 'cancelling'}
              >
                {scan.phase === 'cancelling' ? '正在停止…' : '取消扫描…'}
              </button>
            )}
            {!isFullGate && isActive && (
              <button className="button-primary" type="button" onClick={onBack}>
                返回图库
              </button>
            )}
            {(scan.phase === 'cancelled' || scan.phase === 'failed') && (
              <button
                className="button-primary"
                type="button"
                onClick={() => void retry()}
                disabled={busy}
              >
                {busy ? '正在启动…' : '继续扫描'}
              </button>
            )}
            {(scan.phase === 'cancelled' || scan.phase === 'failed') && (
              <button type="button" onClick={onOpenSettings}>
                重新选择照片文件夹
              </button>
            )}
            {scan.phase === 'completed' && (
              <button
                className="button-primary"
                type="button"
                onClick={onEnterLibrary}
              >
                {isFullGate ? '进入图库' : '返回图库'}
              </button>
            )}
          </div>
        </aside>
      </main>
      <footer className="status-bar tool-status">
        <span>{phaseLabel(scan.phase)}</span>
        <span>扫描只读取照片文件</span>
        <span className="readonly-status">导入与删除保持关闭</span>
      </footer>
      {confirmCancel && (
        <Modal
          title="取消扫描？"
          confirmLabel="确认取消扫描"
          busy={busy}
          onConfirm={() => void cancel()}
          onCancel={() => {
            setConfirmCancel(false);
            window.requestAnimationFrame(() =>
              cancelButtonRef.current?.focus(),
            );
          }}
        >
          <p>
            取消会停止继续枚举。已安全提交的只读索引会保留，但不会被当作完整图库。
          </p>
          <p className="safe-note">
            取消不会撤销照片文件夹绑定，也不会修改任何照片。
          </p>
        </Modal>
      )}
    </div>
  );
}

function Metric({
  label,
  value,
  warning = false,
}: {
  label: string;
  value: number;
  warning?: boolean;
}) {
  return (
    <div className={warning ? 'scan-metric is-warning' : 'scan-metric'}>
      <strong>{value.toLocaleString('zh-CN')}</strong>
      <span>{label}</span>
    </div>
  );
}

function scanTitle(scan: ScanStatus): string {
  if (scan.phase === 'running')
    return scan.kind === 'full' ? '正在扫描照片' : '正在更新图库';
  if (scan.phase === 'cancelling') return '正在安全停止扫描';
  if (scan.phase === 'cancelled') return '扫描已取消';
  if (scan.phase === 'failed') return '扫描未完成';
  return '扫描完成';
}

function scanDescription(scan: ScanStatus): string {
  if (scan.phase === 'completed')
    return `已发现 ${scan.discoveredAssets.toLocaleString('zh-CN')} 个照片文件，并保存本次扫描报告。`;
  if (scan.phase === 'cancelled')
    return '部分索引已保留。继续扫描或重新选择照片文件夹后再进入完整图库。';
  if (scan.phase === 'failed')
    return '既有索引没有被删除；请处理连接问题后继续扫描。';
  if (scan.phase === 'cancelling')
    return '正在等待当前只读文件检查结束，不会强行中断正在读取的照片。';
  return '正在递归检查 JPG、JPEG 与 RAF；不会估算不可靠的剩余时间。';
}

function phaseLabel(phase: ScanStatus['phase']): string {
  return {
    running: '扫描进行中',
    cancelling: '正在取消',
    cancelled: '已取消',
    failed: '失败',
    completed: '完成',
  }[phase];
}

function readError(reason: unknown): string {
  if (typeof reason === 'object' && reason !== null && 'userMessage' in reason)
    return String(reason.userMessage);
  return reason instanceof Error ? reason.message : '扫描操作失败。';
}
