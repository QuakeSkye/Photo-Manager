import { useEffect, useRef, useState, type RefObject } from 'react';

import {
  bindLibraryFolder,
  chooseLibraryFolder,
  validateLibraryFolder,
  type BindResult,
  type LibraryCandidate,
  type LibrarySnapshot,
} from '../../shared/commands/library';
import {
  chooseCacheFolder,
  choosePhotoshop,
  getCacheSettings,
  getPhotoshopStatus,
  updateCacheCapacity,
  updateCacheLocation,
  validateCacheLocation,
  type CacheLocationCandidate,
  type CacheSettings,
  type PhotoshopStatus,
} from '../../shared/commands/settings';
import { Modal } from '../../shared/ui/Modal';
import { TopBar } from '../../shared/ui/TopBar';
import type { SafetyStatus } from '../../shared/commands/safety';

interface SettingsViewProps {
  readonly snapshot: LibrarySnapshot;
  readonly isBindingGate: boolean;
  readonly settingsButtonRef: RefObject<HTMLButtonElement | null>;
  readonly onBack: () => void;
  readonly onBindingStarted: (result: BindResult) => void;
  readonly safetyStatus?: SafetyStatus | null;
}

export function SettingsView({
  snapshot,
  isBindingGate,
  settingsButtonRef,
  onBack,
  onBindingStarted,
  safetyStatus = null,
}: SettingsViewProps) {
  const [section, setSection] = useState<
    'folder' | 'cache' | 'photoshop' | 'reports'
  >('folder');
  const [candidate, setCandidate] = useState<LibraryCandidate | null>(null);
  const [cache, setCache] = useState<CacheSettings | null>(null);
  const [photoshop, setPhotoshop] = useState<PhotoshopStatus | null>(null);
  const [cacheChange, setCacheChange] = useState<
    | { type: 'location'; candidate: CacheLocationCandidate }
    | { type: 'capacity'; capacityBytes: number }
    | null
  >(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const chooseButtonRef = useRef<HTMLButtonElement>(null);
  const scanActive =
    snapshot.scan?.phase === 'running' || snapshot.scan?.phase === 'cancelling';

  useEffect(() => {
    void Promise.all([getCacheSettings(), getPhotoshopStatus()])
      .then(([nextCache, nextPhotoshop]) => {
        setCache(nextCache);
        setPhotoshop(nextPhotoshop);
      })
      .catch((reason) => setError(readError(reason)));
  }, []);

  const choose = async () => {
    setError(null);
    setSaved(false);
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
      setSaved(true);
      onBindingStarted(result);
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setBusy(false);
    }
  };

  const selectCacheLocation = async () => {
    setError(null);
    setSaved(false);
    try {
      const path = await chooseCacheFolder();
      if (path === null) return;
      const next = await validateCacheLocation(path);
      setCacheChange({ type: 'location', candidate: next });
    } catch (reason) {
      setError(readError(reason));
    }
  };

  const confirmCacheChange = async () => {
    if (cacheChange === null) return;
    setBusy(true);
    setError(null);
    try {
      const next =
        cacheChange.type === 'location'
          ? await updateCacheLocation(
              cacheChange.candidate.displayPath,
              crypto.randomUUID(),
            )
          : await updateCacheCapacity(
              cacheChange.capacityBytes,
              crypto.randomUUID(),
            );
      setCache(next);
      setCacheChange(null);
      setSaved(true);
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setBusy(false);
    }
  };

  const selectPhotoshop = async () => {
    setBusy(true);
    setError(null);
    setSaved(false);
    try {
      const next = await choosePhotoshop();
      if (next !== null) {
        setPhotoshop(next);
        setSaved(true);
      }
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
        onOpenLibrary={isBindingGate ? undefined : onBack}
        onOpenSettings={() => undefined}
        onBack={isBindingGate ? undefined : onBack}
      />
      <main className="tool-workspace">
        <nav className="tool-sidebar" aria-label="设置分组">
          <p className="sidebar-title">设置</p>
          <button
            className={
              section === 'cache' ? 'sidebar-item is-active' : 'sidebar-item'
            }
            type="button"
            onClick={() => setSection('cache')}
          >
            <span aria-hidden="true">◫</span> 预览缓存
          </button>
          <button
            className={
              section === 'photoshop'
                ? 'sidebar-item is-active'
                : 'sidebar-item'
            }
            type="button"
            onClick={() => setSection('photoshop')}
          >
            <span aria-hidden="true">Ps</span> Photoshop
          </button>
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
          <div className="continuous-panel" aria-label="写入安全状态">
            <div className="setting-row">
              <div className="setting-copy">
                <span className="setting-label">
                  写入安全状态 · {safetyStatus?.phase ?? '正在读取'}
                </span>
                <strong>
                  {safetyStatus?.globalWriteGate === 'open'
                    ? 'Import 与 Recycle 已开放'
                    : 'Import 与 Recycle 已关闭'}
                </strong>
                <span role="status">
                  {safetyStatus?.message ??
                    '正在读取实时数据库与恢复协调状态。'}
                </span>
                {safetyStatus !== null && (
                  <span>
                    Import：
                    {safetyStatus.importGate === 'open' ? '开放' : '关闭'} ·
                    Recycle：
                    {safetyStatus.recycleGate === 'open' ? '开放' : '关闭'} ·
                    数据库：
                    {safetyStatus.databaseMode}
                  </span>
                )}
              </div>
            </div>
          </div>
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
          ) : section === 'cache' ? (
            <CachePanel
              cache={cache}
              disabled={scanActive || busy}
              onChooseLocation={() => void selectCacheLocation()}
              onChooseCapacity={(capacityBytes) =>
                setCacheChange({ type: 'capacity', capacityBytes })
              }
            />
          ) : section === 'photoshop' ? (
            <PhotoshopPanel
              status={photoshop}
              busy={busy}
              onChoose={() => void selectPhotoshop()}
            />
          ) : (
            <OperationRecords reports={snapshot.reports} />
          )}
          {saved && <p className="settings-success">当前设置已保存。</p>}
          {error !== null && section !== 'folder' && (
            <div className="inline-error settings-error" role="alert">
              <strong>设置未能更改</strong>
              <span>{error} 原设置未更改。</span>
            </div>
          )}
        </section>
        <aside className="tool-summary">
          <div>
            <p className="section-label">当前状态</p>
            <strong>{summaryTitle(section, snapshot, cache, photoshop)}</strong>
            <p>{summaryCopy(section, snapshot, cache, photoshop)}</p>
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
      {cacheChange !== null && (
        <Modal
          title={
            cacheChange.type === 'location'
              ? '更改预览缓存位置？'
              : '更改预览缓存上限？'
          }
          confirmLabel="确认更改"
          busy={busy}
          onConfirm={() => void confirmCacheChange()}
          onCancel={() => setCacheChange(null)}
        >
          {cacheChange.type === 'location' ? (
            <>
              <p className="modal-path">{cacheChange.candidate.displayPath}</p>
              <p>
                新预览会在新位置按需生成。旧缓存不会搬迁，只会在应用能证明归属和文件身份时于后台渐进淘汰。
              </p>
            </>
          ) : (
            <>
              <p>
                旧上限：{formatBytes(cache?.capacityBytes ?? 0)}；新上限：
                {formatBytes(cacheChange.capacityBytes)}。
              </p>
              <p>
                {cache !== null &&
                cacheChange.capacityBytes < cache.capacityBytes
                  ? '缩小后会在后台渐进淘汰较旧预览，页面不需要等待。'
                  : '增大的是容量上限，不会立即占用磁盘空间。'}
              </p>
            </>
          )}
          <p className="safe-note">缓存不是照片备份；淘汰缓存不会删除照片。</p>
        </Modal>
      )}
    </div>
  );
}

function CachePanel({
  cache,
  disabled,
  onChooseLocation,
  onChooseCapacity,
}: {
  cache: CacheSettings | null;
  disabled: boolean;
  onChooseLocation: () => void;
  onChooseCapacity: (capacityBytes: number) => void;
}) {
  const gib = 1_073_741_824;
  return (
    <>
      <div className="content-heading">
        <p className="section-label">预览缓存</p>
        <h1>按需生成的预览</h1>
        <p>
          缓存由应用自动按 LRU
          淘汰，不会预先占满。缓存不是照片备份，淘汰缓存不会删除照片。
        </p>
      </div>
      <div className="continuous-panel cache-panel">
        <div className="setting-row">
          <div className="setting-copy">
            <span className="setting-label">缓存父目录</span>
            <strong className="long-path">
              {cache?.parentDisplayPath ?? '正在读取…'}
            </strong>
            <span className="connection-state is-online">
              {cache?.activeAvailable === false
                ? '缓存暂时不可用，将按需重新生成'
                : '可用'}
            </span>
          </div>
          <button type="button" disabled={disabled} onClick={onChooseLocation}>
            更改位置…
          </button>
        </div>
        <div className="cache-metrics" aria-label="缓存空间摘要">
          <Metric label="实际占用" value={formatBytes(cache?.usedBytes ?? 0)} />
          <Metric
            label="当前上限"
            value={formatBytes(cache?.capacityBytes ?? 32 * gib)}
          />
          <Metric
            label="磁盘可用"
            value={
              cache?.availableBytes === null || cache === null
                ? '暂时不可用'
                : formatBytes(cache.availableBytes)
            }
          />
        </div>
        <div className="setting-row compact-setting-row">
          <div className="setting-copy">
            <span className="setting-label">容量上限</span>
            <strong>大于 0，最高 64 GB</strong>
            {cache !== null && cache.drainingRootCount > 0 && (
              <span className="row-note-inline">
                {cache.drainingRootCount} 个旧位置正在安全渐进淘汰（
                {formatBytes(cache.drainingBytes)}）
              </span>
            )}
          </div>
          <select
            aria-label="预览缓存容量上限"
            value={cache?.capacityBytes ?? 32 * gib}
            disabled={cache === null || disabled}
            onChange={(event) => onChooseCapacity(Number(event.target.value))}
          >
            {[8, 16, 32, 48, 64].map((value) => (
              <option key={value} value={value * gib}>
                {value} GB
              </option>
            ))}
          </select>
        </div>
      </div>
      {disabled && (
        <p className="row-note standalone-note" role="status">
          请先完成或取消当前图库扫描，再更改缓存设置。
        </p>
      )}
    </>
  );
}

function PhotoshopPanel({
  status,
  busy,
  onChoose,
}: {
  status: PhotoshopStatus | null;
  busy: boolean;
  onChoose: () => void;
}) {
  return (
    <>
      <div className="content-heading">
        <p className="section-label">Photoshop</p>
        <h1>外部 RAW 编辑器</h1>
        <p>
          仅注册经过验证的
          Photoshop.exe。界面保存工具引用，不把程序路径作为后续授权。
        </p>
      </div>
      <div className="continuous-panel">
        <div className="setting-row">
          <div className="setting-copy">
            <span className="setting-label">Adobe Photoshop</span>
            <strong>{status?.executableLabel ?? '尚未选择 Photoshop'}</strong>
            <span
              className={
                status?.available
                  ? 'connection-state is-online'
                  : 'connection-state'
              }
            >
              {status?.message ?? '正在验证配置…'}
            </span>
          </div>
          <button type="button" disabled={busy} onClick={onChoose}>
            {status?.configured ? '重新选择 Photoshop' : '选择 Photoshop'}
          </button>
        </div>
      </div>
      <div className="external-tool-note">
        <strong>外部程序边界</strong>
        <p>
          文件交给 Photoshop 后，其修改、保存和附属文件由 Photoshop
          管理；本应用不监控也不控制 Photoshop 后续写入。
        </p>
      </div>
    </>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function summaryTitle(
  section: 'folder' | 'cache' | 'photoshop' | 'reports',
  snapshot: LibrarySnapshot,
  cache: CacheSettings | null,
  photoshop: PhotoshopStatus | null,
): string {
  if (section === 'cache')
    return cache?.activeAvailable === false ? '缓存暂时离线' : '缓存可用';
  if (section === 'photoshop')
    return photoshop?.available ? 'Photoshop 已验证' : '需要选择程序';
  if (section === 'reports') return `${snapshot.reports.length} 条操作记录`;
  return snapshot.binding === null
    ? '等待选择图库'
    : bindingStatus(snapshot.binding.status);
}

function summaryCopy(
  section: 'folder' | 'cache' | 'photoshop' | 'reports',
  snapshot: LibrarySnapshot,
  cache: CacheSettings | null,
  photoshop: PhotoshopStatus | null,
): string {
  if (section === 'cache')
    return cache?.activeAvailable === false
      ? '照片仍可浏览；预览会在位置恢复后按需生成。'
      : '新预览写入当前 Active Root；旧 Root 仅在身份可证明时淘汰。';
  if (section === 'photoshop')
    return photoshop?.message ?? '正在读取 Photoshop 配置。';
  if (section === 'reports') return '历史事实只读保留，不能编辑或清空。';
  return snapshot.binding === null
    ? '确认前不会建立图库上下文。'
    : '扫描照片时只读取源文件；索引保存在应用数据中。';
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

function formatBytes(bytes: number): string {
  if (bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit < 2 ? 0 : 1)} ${units[unit]}`;
}

function readError(reason: unknown): string {
  if (typeof reason === 'object' && reason !== null && 'userMessage' in reason)
    return String(reason.userMessage);
  return reason instanceof Error ? reason.message : '文件夹验证失败。';
}
