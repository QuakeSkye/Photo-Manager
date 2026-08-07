import { useId, useState, type ReactNode } from 'react';

import type { PhotoDetail } from '../../shared/commands/library';
import type { PhotoshopStatus } from '../../shared/commands/settings';

interface PhotoInspectorProps {
  readonly detail: PhotoDetail | null;
  readonly loading: boolean;
  readonly error: string | null;
  readonly marked: boolean;
  readonly mutationPending: boolean;
  readonly toolStatus: PhotoshopStatus | null;
  readonly externalActionPending: boolean;
  readonly externalError: string | null;
  readonly onOpenPhotoshop: () => void;
  readonly onOpenFileLocation: () => void;
  readonly onOpenSettings: () => void;
  readonly onToggleDeletion: () => void;
}

export function PhotoInspector({
  detail,
  loading,
  error,
  marked,
  mutationPending,
  toolStatus,
  externalActionPending,
  externalError,
  onOpenPhotoshop,
  onOpenFileLocation,
  onOpenSettings,
  onToggleDeletion,
}: PhotoInspectorProps) {
  const [timeExpanded, setTimeExpanded] = useState(true);
  const [parametersExpanded, setParametersExpanded] = useState(true);

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
            <MetadataAccordion
              title="时间与文件"
              expanded={timeExpanded}
              onToggle={() => setTimeExpanded((current) => !current)}
            >
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
                    <MetadataRow
                      label="文件时间"
                      value={formatFileTime(detail.fileTimeMs)}
                    />
                    <p className="metadata-note">按相机记录的本地时间显示</p>
                  </>
                )}
                <MetadataRow
                  label="容量"
                  value={formatBytes(detail.totalSizeBytes)}
                />
              </dl>
            </MetadataAccordion>
            <MetadataAccordion
              title="照片参数"
              summary={detail.cameraModel ?? '未记录'}
              expanded={parametersExpanded}
              onToggle={() => setParametersExpanded((current) => !current)}
            >
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
              </dl>
            </MetadataAccordion>
          </>
        )}
      </div>
      <div className="inspector-spacer" />
      <section className="library-actions" aria-label="图库动作区">
        {externalError !== null && (
          <div className="inline-error compact-error" role="alert">
            <strong>外部工具操作未完成</strong>
            <span>{externalError}</span>
          </div>
        )}
        <button
          className="button-primary"
          type="button"
          disabled={
            detail?.raf === null ||
            detail === null ||
            externalActionPending ||
            (toolStatus?.configured === true && !toolStatus.available)
          }
          onClick={onOpenPhotoshop}
        >
          {externalActionPending ? '正在准备…' : '用 Photoshop 打开 RAF'}
        </button>
        {detail?.raf === null && (
          <p>当前照片没有 RAF，Photoshop 操作不可用。</p>
        )}
        {detail?.raf !== null &&
          toolStatus?.configured === true &&
          !toolStatus.available && (
            <p>
              Photoshop 需要重新选择。{' '}
              <button
                className="inline-link"
                type="button"
                onClick={onOpenSettings}
              >
                打开设置
              </button>
            </p>
          )}
        <button
          className="secondary-button"
          type="button"
          disabled={detail === null || externalActionPending}
          onClick={onOpenFileLocation}
        >
          打开当前文件位置
        </button>
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

function MetadataAccordion({
  title,
  summary,
  expanded,
  onToggle,
  children,
}: {
  title: string;
  summary?: string;
  expanded: boolean;
  onToggle: () => void;
  children: ReactNode;
}) {
  const contentId = useId();

  return (
    <section className="metadata-section metadata-accordion">
      <h3 className="metadata-accordion-heading">
        <button
          className="metadata-accordion-toggle"
          type="button"
          aria-controls={contentId}
          aria-expanded={expanded}
          onClick={onToggle}
        >
          <span>{title}</span>
          {summary !== undefined && (
            <span className="metadata-accordion-summary">{summary}</span>
          )}
          <span className="metadata-accordion-chevron" aria-hidden="true">
            {expanded ? '−' : '+'}
          </span>
        </button>
      </h3>
      <div
        id={contentId}
        className="metadata-accordion-content"
        hidden={!expanded}
      >
        {children}
      </div>
    </section>
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
