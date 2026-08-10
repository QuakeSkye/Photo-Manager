import { useId, useState, type ReactNode } from 'react';

import type {
  FilmSimulationValue,
  PhotoDetail,
  RationalValue,
} from '../../shared/commands/library';
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
  readonly onUndoDeletion?: () => void;
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
  onUndoDeletion,
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
                {detail.captureLocalKey !== null && (
                  <>
                    <MetadataRow
                      label="拍摄日期"
                      value={detail.captureLocalKey.slice(0, 10)}
                    />
                    <MetadataRow
                      label="相机本地时间"
                      value={detail.captureLocalKey.slice(11, 19)}
                    />
                  </>
                )}
                <MetadataRow
                  label="照片大小"
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
                <MetadataRow
                  label="镜头"
                  value={detail.lensModel ?? '未记录'}
                  valueClassName="lens-value"
                />
                <MetadataRow
                  label="光圈"
                  value={formatFNumber(detail.fNumber)}
                />
                <MetadataRow
                  label="快门"
                  value={formatExposureTime(detail.exposureTime)}
                />
                <MetadataRow
                  label="ISO"
                  value={detail.iso === null ? '未记录' : `ISO ${detail.iso}`}
                />
                <MetadataRow
                  label="焦距"
                  value={formatFocalLength(detail.focalLength)}
                />
                <MetadataRow
                  label="尺寸"
                  value={formatDimensions(
                    detail.pixelWidth,
                    detail.pixelHeight,
                  )}
                />
                <MetadataRow
                  label="胶片模拟"
                  value={formatFilmSimulation(detail.filmSimulation)}
                />
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
          className="button-primary photoshop-action"
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
        <p className="inspector-shortcuts">
          ← → 浏览 · <kbd className="keycap-centered">D</kbd> 标记 ·{' '}
          <button
            className="inline-link"
            type="button"
            onClick={onUndoDeletion}
          >
            <kbd>R</kbd> 撤销
          </button>
        </p>
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
  valueClassName,
}: {
  label: string;
  value: string;
  emphasis?: boolean;
  valueClassName?: string;
}) {
  return (
    <div className={emphasis ? 'metadata-row is-emphasis' : 'metadata-row'}>
      <dt>{label}</dt>
      <dd className={valueClassName}>{value}</dd>
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

function formatFNumber(value: RationalValue | null): string {
  const decimal = rationalDecimal(value);
  return decimal === null ? '未记录' : `F${formatDecimal(decimal, 2)}`;
}

function formatExposureTime(value: RationalValue | null): string {
  const seconds = rationalDecimal(value);
  if (seconds === null || seconds <= 0) return '未记录';
  if (seconds < 1) {
    const inverse = 1 / seconds;
    const rounded = Math.round(inverse);
    if (Math.abs(inverse - rounded) < 0.000_001) return `1/${rounded} 秒`;
    return `${formatDecimal(seconds, 4)} 秒`;
  }
  return `${formatDecimal(seconds, 2)} 秒`;
}

function formatFocalLength(value: RationalValue | null): string {
  const decimal = rationalDecimal(value);
  return decimal === null ? '未记录' : `${formatDecimal(decimal, 1)} mm`;
}

function formatDimensions(width: number | null, height: number | null): string {
  if (width === null || height === null || width <= 0 || height <= 0)
    return '未记录';
  return `${width} × ${height}`;
}

function rationalDecimal(value: RationalValue | null): number | null {
  if (value === null || value.denominator === 0) return null;
  const decimal = value.numerator / value.denominator;
  return Number.isFinite(decimal) ? decimal : null;
}

function formatDecimal(value: number, maximumFractionDigits: number): string {
  return new Intl.NumberFormat('en-US', {
    useGrouping: false,
    maximumFractionDigits,
  }).format(value);
}

const FILM_SIMULATION_LABELS: Readonly<Record<string, string>> = {
  provia: 'Provia',
  velvia: 'Velvia',
  astia: 'Astia',
  'pro-neg-std': 'Pro Neg. Std',
  'pro-neg-hi': 'Pro Neg. Hi',
  'classic-chrome': 'Classic Chrome',
  eterna: 'Eterna',
  'classic-negative': 'Classic Negative',
  'eterna-bleach-bypass': 'Eterna Bleach Bypass',
  'nostalgic-neg': 'Nostalgic Neg',
  'reala-ace': 'Reala ACE',
  monochrome: 'Monochrome',
  'monochrome-red': 'Monochrome + R',
  'monochrome-yellow': 'Monochrome + Ye',
  'monochrome-green': 'Monochrome + G',
  sepia: 'Sepia',
  acros: 'Acros',
  'acros-red': 'Acros + R',
  'acros-yellow': 'Acros + Ye',
  'acros-green': 'Acros + G',
};

function formatFilmSimulation(value: FilmSimulationValue | null): string {
  if (value === null) return '未记录';
  const label = FILM_SIMULATION_LABELS[value.key];
  if (label !== undefined) return label;
  if (value.code === null) return '未知模式';
  return `未知模式（0x${value.code.toString(16).toUpperCase()}）`;
}
