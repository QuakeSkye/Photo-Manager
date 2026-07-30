import { useRef, useState, type RefObject } from 'react';

import {
  bindLibraryFolder,
  chooseLibraryFolder,
  validateLibraryFolder,
  type BindResult,
  type LibraryCandidate,
  type LibrarySnapshot,
} from '../../shared/commands/library';
import { Modal } from '../../shared/ui/Modal';
import { TopBar } from '../../shared/ui/TopBar';

interface SettingsViewProps {
  readonly snapshot: LibrarySnapshot;
  readonly isBindingGate: boolean;
  readonly settingsButtonRef: RefObject<HTMLButtonElement | null>;
  readonly onBack: () => void;
  readonly onBindingStarted: (result: BindResult) => void;
}

export function SettingsView({
  snapshot,
  isBindingGate,
  settingsButtonRef,
  onBack,
  onBindingStarted,
}: SettingsViewProps) {
  const [section, setSection] = useState<'folder' | 'reports'>('folder');
  const [candidate, setCandidate] = useState<LibraryCandidate | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const chooseButtonRef = useRef<HTMLButtonElement>(null);
  const scanActive =
    snapshot.scan?.phase === 'running' || snapshot.scan?.phase === 'cancelling';

  const choose = async () => {
    setError(null);
    try {
      const path = await chooseLibraryFolder();
      if (path === null) return;
      setCandidate(await validateLibraryFolder(path));
    } catch (reason) {
      setError(readError(reason));
    }
  };

  const confirm = async () => {
    if (candidate === null) return;
    setBusy(true);
    setError(null);
    try {
      const result = await bindLibraryFolder(candidate.displayPath);
      setCandidate(null);
      onBindingStarted(result);
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setBusy(false);
    }
  };

  const closeCandidate = () => {
    setCandidate(null);
    window.requestAnimationFrame(() => chooseButtonRef.current?.focus());
  };

  return (
    <div className="screen tool-screen settings-screen">
      <TopBar
        context={isBindingGate ? '首次设置' : '设置'}
        settingsButtonRef={settingsButtonRef}
        onOpenSettings={() => undefined}
        onBack={isBindingGate ? undefined : onBack}
      />
      <main className="tool-workspace">
        <nav className="tool-sidebar" aria-label="设置分组">
          <p className="sidebar-title">设置</p>
          <button
            className={
              section === 'folder' ? 'sidebar-item is-active' : 'sidebar-item'
            }
            type="button"
            onClick={() => setSection('folder')}
          >
            <span aria-hidden="true">▣</span> 照片文件夹
          </button>
          <button
            className={
              section === 'reports' ? 'sidebar-item is-active' : 'sidebar-item'
            }
            type="button"
            onClick={() => setSection('reports')}
          >
            <span aria-hidden="true">≡</span> 操作记录
          </button>
        </nav>
        <section className="tool-content">
          {section === 'folder' ? (
            <>
              <div className="content-heading">
                <p className="section-label">照片文件夹</p>
                <h1>{isBindingGate ? '选择照片文件夹' : 'Fuji 图库根目录'}</h1>
                <p>
                  {isBindingGate
                    ? '选择后将先进行完全只读的全量扫描；完成前不会显示虚假的空图库。'
                    : '照片仅从这里读取。更改位置会先验证身份并明确说明影响。'}
                </p>
              </div>
              <div className="continuous-panel">
                <div className="setting-row">
                  <div className="setting-copy">
                    <span className="setting-label">当前照片文件夹</span>
                    {snapshot.binding === null ? (
                      <strong className="path-empty">尚未选择</strong>
                    ) : (
                      <strong
                        className="long-path"
                        title={snapshot.binding.displayPath}
                      >
                        {snapshot.binding.displayPath}
                      </strong>
                    )}
                    <span
                      className={
                        snapshot.binding?.status === 'Online'
                          ? 'connection-state is-online'
                          : 'connection-state'
                      }
                    >
                      {snapshot.binding === null
                        ? '需要选择'
                        : bindingStatus(snapshot.binding.status)}
                    </span>
                  </div>
                  <button
                    ref={chooseButtonRef}
                    type="button"
                    onClick={() => void choose()}
                    disabled={scanActive}
                  >
                    {snapshot.binding === null ? '选择文件夹' : '重新选择'}
                  </button>
                </div>
                {scanActive && (
                  <p className="row-note" role="status">
                    请先完成或取消当前图库扫描，再更改照片文件夹。
                  </p>
                )}
                {error !== null && (
                  <div className="inline-error" role="alert">
                    <strong>无法使用此目录</strong>
                    <span>{error} 原设置未更改。</span>
                  </div>
                )}
              </div>
            </>
          ) : (
            <OperationRecords reports={snapshot.reports} />
          )}
        </section>
        <aside className="tool-summary">
          <div>
            <p className="section-label">当前状态</p>
            <strong>
              {snapshot.binding === null
                ? '等待选择图库'
                : bindingStatus(snapshot.binding.status)}
            </strong>
            <p>
              {snapshot.binding === null
                ? '确认前不会建立图库上下文。'
                : '扫描照片时只读取源文件；索引保存在应用数据中。'}
            </p>
          </div>
          <div className="summary-spacer" />
          {!isBindingGate && (
            <button className="button-primary" type="button" onClick={onBack}>
              返回图库
            </button>
          )}
        </aside>
      </main>
      <footer className="status-bar tool-status">
        <span>设置</span>
        <span>每项设置单独验证并确认</span>
        <span className="readonly-status">只读照片文件</span>
      </footer>
      {candidate !== null && (
        <Modal
          title={
            candidate.reconnectsExistingLibrary
              ? '重新连接原图库'
              : '切换到新的 Fuji 图库'
          }
          confirmLabel={
            candidate.reconnectsExistingLibrary
              ? '确认重新连接'
              : '确认切换并扫描'
          }
          busy={busy}
          onConfirm={() => void confirm()}
          onCancel={closeCandidate}
        >
          <p className="modal-path">{candidate.displayPath}</p>
          <p>
            {candidate.reconnectsExistingLibrary
              ? '已可靠识别为原图库。浏览位置、索引和历史会保留，并启动增量扫描。'
              : '这不是已知的原图库。旧图库状态会保留但不会迁移；新图库需要完成全量扫描后才能进入。'}
          </p>
          <p className="safe-note">
            扫描只读取照片，不会修改、移动或删除任何照片文件。
          </p>
        </Modal>
      )}
    </div>
  );
}

function OperationRecords({
  reports,
}: {
  reports: LibrarySnapshot['reports'];
}) {
  return (
    <>
      <div className="content-heading">
        <p className="section-label">操作记录</p>
        <h1>扫描报告</h1>
        <p>记录只读展示，不能编辑或清空。</p>
      </div>
      <div className="records-list">
        {reports.length === 0 ? (
          <div className="empty-records">还没有扫描记录</div>
        ) : (
          reports.map((report) => (
            <article
              className="record-row"
              key={`${report.startedAt}-${report.result ?? 'open'}`}
            >
              <div className="record-icon" aria-hidden="true">
                ↻
              </div>
              <div>
                <strong>图库扫描</strong>
                <span>{formatMicros(report.startedAt)}</span>
              </div>
              <span>
                {(report.itemCount ?? 0).toLocaleString('zh-CN')} 个项目
              </span>
              <span
                className={`report-state state-${report.result?.toLowerCase() ?? 'running'}`}
              >
                {reportResult(report.result)}
              </span>
            </article>
          ))
        )}
      </div>
    </>
  );
}

function formatMicros(value: number): string {
  return new Intl.DateTimeFormat('zh-CN', {
    dateStyle: 'medium',
    timeStyle: 'short',
    hour12: false,
  }).format(new Date(value / 1000));
}

function reportResult(result: string | null): string {
  if (result === 'Success') return '完成';
  if (result === 'Partial') return '部分完成';
  if (result === 'Cancelled') return '已取消';
  if (result === 'Failed') return '失败';
  return '进行中';
}

function bindingStatus(status: string): string {
  if (status === 'Online') return '已连接';
  if (status === 'Offline') return '离线';
  return '需要重新选择';
}

function readError(reason: unknown): string {
  if (typeof reason === 'object' && reason !== null && 'userMessage' in reason)
    return String(reason.userMessage);
  return reason instanceof Error ? reason.message : '文件夹验证失败。';
}
