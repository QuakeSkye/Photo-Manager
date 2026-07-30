import type { PhotoDetail } from '../../shared/commands/library';

interface PhotoInspectorProps {
  readonly detail: PhotoDetail | null;
  readonly loading: boolean;
  readonly error: string | null;
  readonly marked: boolean;
  readonly mutationPending: boolean;
  readonly onToggleDeletion: () => void;
}

export function PhotoInspector({
  detail,
  loading,
  error,
  marked,
  mutationPending,
  onToggleDeletion,
}: PhotoInspectorProps) {
  return (
    <aside className="photo-inspector" aria-label="照片详情">
      <div className="inspector-scroll">
        {loading && <InspectorSkeleton />}
        {error !== null && (
          <div className="inline-error" role="alert">
            <strong>无法读取照片信息</strong>
            <span>{error}</span>
          </div>
        )}
        {detail !== null && (
          <>
            <section className="photo-identity">
              <p className="section-label">当前照片</p>
              <h2 title={detail.fileName}>{detail.fileName}</h2>
              <div className="format-badges" aria-label="照片格式">
                <span
                  className={
                    detail.jpeg === null
                      ? 'format-badge is-missing'
                      : 'format-badge'
                  }
                >
                  JPG {detail.jpeg === null ? '×' : '✓'}
                </span>
                <span
                  className={
                    detail.raf === null
                      ? 'format-badge is-missing'
                      : 'format-badge'
                  }
                >
                  RAF {detail.raf === null ? '×' : '✓'}
                </span>
              </div>
            </section>
            <section className="metadata-section">
              <h3>时间</h3>
              <dl>
                {detail.captureLocalKey === null ? (
                  <>
                    <MetadataRow
                      label="拍摄时间"
                      value="拍摄时间未知"
                      emphasis
                    />
                    <MetadataRow
                      label="文件时间"
                      value={formatFileTime(detail.fileTimeMs)}
                    />
                    <p className="metadata-note">
                      文件时间仅供参考，不参与照片排序。
                    </p>
                  </>
                ) : (
                  <>
                    <MetadataRow
                      label="拍摄日期"
                      value={detail.captureLocalKey.slice(0, 10)}
                    />
                    <MetadataRow
                      label="相机本地时间"
                      value={detail.captureLocalKey.slice(11, 19)}
                    />
                    <p className="metadata-note">按相机记录的本地时间显示</p>
                  </>
                )}
              </dl>
            </section>
            <section className="metadata-section">
              <h3>照片参数</h3>
              <dl>
                <MetadataRow
                  label="相机"
                  value={detail.cameraModel ?? '未记录'}
                />
                <MetadataRow label="镜头" value="未记录" />
                <MetadataRow label="光圈" value="未记录" />
                <MetadataRow label="快门" value="未记录" />
                <MetadataRow label="ISO" value="未记录" />
                <MetadataRow label="焦距" value="未记录" />
                <MetadataRow label="尺寸" value="未记录" />
                <MetadataRow
                  label="容量"
                  value={formatBytes(detail.totalSizeBytes)}
                />
              </dl>
            </section>
          </>
        )}
      </div>
      <div className="inspector-spacer" />
      <section className="library-actions" aria-label="图库动作区">
        <button
          className="delete-outline-button"
          type="button"
          disabled={detail === null || mutationPending}
          onClick={onToggleDeletion}
        >
          {mutationPending ? '正在提交…' : marked ? '取消待删除' : '标记待删除'}
          <kbd>D</kbd>
        </button>
        <p>D 只保存复核标记，不会移动或修改照片。</p>
      </section>
    </aside>
  );
}

function MetadataRow({
  label,
  value,
  emphasis = false,
}: {
  label: string;
  value: string;
  emphasis?: boolean;
}) {
  return (
    <div className={emphasis ? 'metadata-row is-emphasis' : 'metadata-row'}>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

function InspectorSkeleton() {
  return (
    <div className="inspector-skeleton" aria-label="正在加载照片信息">
      <i />
      <i />
      <i />
      <i />
      <i />
    </div>
  );
}

function formatBytes(bytes: number): string {
  if (bytes <= 0) return '未记录';
  const units = ['B', 'KB', 'MB', 'GB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit < 2 ? 0 : 1)} ${units[unit]}`;
}

function formatFileTime(value: number | null): string {
  if (value === null) return '未记录';
  return new Intl.DateTimeFormat('zh-CN', {
    dateStyle: 'medium',
    timeStyle: 'medium',
    hour12: false,
  }).format(new Date(value));
}
