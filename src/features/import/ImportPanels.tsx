import { useMemo, useState } from 'react';
import type {
  ImportDraftItem,
  ImportExecutionStatus,
  ImportPreflightDraft,
  ImportPreflightStatus,
  ImportRecoverySummary,
} from '../../shared/commands/import';

type PlanFilter =
  'all' | 'new' | 'skip' | 'conflict' | 'unknown' | 'unreadable';

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const index = Math.min(
    units.length - 1,
    Math.floor(Math.log(bytes) / Math.log(1024)),
  );
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

export function executionPhaseLabel(status: ImportExecutionStatus): string {
  const labels: Record<ImportExecutionStatus['phase'], string> = {
    idle: '等待开始',
    validatingPlan: '正在检查导入计划',
    awaitingPlanConfirmation: '导入计划有变化',
    awaitingSpace: '目标磁盘空间不足',
    running: '正在导入照片',
    awaitingSource: '等待重新连接来源',
    needsAttention: '有文件需要处理',
    completed: '导入完成',
    completedWithErrors: '导入部分完成',
    cancelled: '导入已取消',
  };
  return labels[status.phase];
}

export function ImportLocations({
  source,
  target,
}: {
  source: string | null;
  target: string | null;
}) {
  return (
    <dl className="import-locations" aria-label="本次导入位置">
      <div>
        <dt>从这里读取</dt>
        <dd>{source ?? '尚未选择来源'}</dd>
      </div>
      <div className="import-location-arrow" aria-hidden="true">
        →
      </div>
      <div>
        <dt>保存到图库</dt>
        <dd>{target ?? '尚未选择图库'}</dd>
      </div>
    </dl>
  );
}

export function PreflightProgress({
  status,
}: {
  status: ImportPreflightStatus;
}) {
  return (
    <section className="import-progress-panel" aria-live="polite">
      <h1>
        {status.phase === 'cancelling' ? '正在停止检查…' : '正在检查导入内容'}
      </h1>
      <p>正在查找照片、核对已有内容并规划保存位置。</p>
      <progress aria-label="检查导入内容进度" />
      <dl className="import-metrics">
        <Metric label="已检查文件" value={status.scannedFiles} />
        <Metric label="发现照片文件" value={status.supportedFiles} />
      </dl>
      <p className="long-path">
        {status.currentRelativePath ?? '正在读取文件夹信息…'}
      </p>
      {status.errorMessage && <p role="alert">{status.errorMessage}</p>}
    </section>
  );
}

export function DraftResults({ draft }: { draft: ImportPreflightDraft }) {
  const [filter, setFilter] = useState<PlanFilter>('all');
  const groups = useMemo(() => {
    const result = new Map<string, ImportDraftItem[]>();
    for (const item of draft.items) {
      const matches =
        filter === 'all' ||
        (filter === 'new' &&
          (item.disposition === 'New' || item.disposition === 'Conflict')) ||
        (filter === 'skip' && item.disposition === 'Skip') ||
        (filter === 'conflict' && item.disposition === 'Conflict') ||
        (filter === 'unknown' && item.dateUnknown) ||
        (filter === 'unreadable' && item.disposition === 'Unreadable');
      if (!matches) continue;
      const items = result.get(item.logicalGroup) ?? [];
      items.push(item);
      result.set(item.logicalGroup, items);
    }
    return result;
  }, [draft.items, filter]);
  const filters: { id: PlanFilter; label: string; count: string }[] = [
    { id: 'all', label: '全部计划', count: `${draft.items.length} 个文件` },
    { id: 'new', label: '将新增', count: `${draft.summary.newFiles} 个文件` },
    {
      id: 'skip',
      label: '已有相同文件',
      count: `${draft.summary.skippedFiles} 个文件`,
    },
    {
      id: 'unreadable',
      label: '无法读取',
      count: `${draft.summary.unreadableFiles} 个文件`,
    },
    {
      id: 'conflict',
      label: '同名另存',
      count: `${draft.summary.conflictGroups} 组`,
    },
    {
      id: 'unknown',
      label: '日期未知',
      count: `${draft.summary.unknownDateFiles} 个文件`,
    },
  ];
  return (
    <section className="import-plan" aria-label="本次导入计划">
      <div className="import-heading">
        <h1>检查导入计划</h1>
        <p>
          将新增 {draft.summary.newFiles} 个文件，合计{' '}
          {formatBytes(draft.summary.newBytes)}。
        </p>
      </div>
      <div
        className="import-plan-filters"
        role="group"
        aria-label="查看计划分类"
      >
        {filters.map(({ id, label, count }) => (
          <button
            key={id}
            type="button"
            aria-pressed={filter === id}
            onClick={() => setFilter(id)}
          >
            <span>{label}</span>
            <strong>{count}</strong>
          </button>
        ))}
      </div>
      <p className="import-muted">
        同名另存属于新增文件；日期未知是附加标签，分类数量不能直接相加。
      </p>
      <div className="import-plan-scope">
        <span>筛选仅用于查看；确认后按完整计划导入。</span>
        {filter !== 'all' && (
          <button
            type="button"
            className="inline-link"
            onClick={() => setFilter('all')}
          >
            显示全部计划
          </button>
        )}
      </div>
      {draft.summary.unreadableFiles > 0 && (
        <p className="import-notice" role="status">
          有 {draft.summary.unreadableFiles}{' '}
          个文件无法读取，本次不会导入。可选择“无法读取”查看原因。
        </p>
      )}
      {groups.size === 0 && (
        <p className="import-plan-empty" role="status">
          此分类没有文件。
        </p>
      )}
      <div className="import-plan-list">
        {Array.from(groups, ([group, items]) => (
          <details
            key={`${filter}:${group}`}
            className="import-plan-group"
            open
          >
            <summary>
              <strong>{group}</strong>
              <span>{items.length} 个文件</span>
            </summary>
            {items.map((item) => (
              <div className="import-plan-item" key={item.sourceRelativePath}>
                <span
                  className={`plan-badge disposition-${item.disposition.toLowerCase()}`}
                >
                  {dispositionLabel(item)}
                </span>
                <div>
                  <strong>
                    {item.fileName}{' '}
                    <small>{item.mediaKind === 'Raf' ? 'RAF' : 'JPG'}</small>
                  </strong>
                  <span className="long-path">
                    来源：{item.sourceRelativePath}
                  </span>
                  {item.targetRelativePath !== null && (
                    <code>
                      {item.disposition === 'Skip' ? '已有文件' : '保存为'}：
                      {item.targetRelativePath}
                    </code>
                  )}
                  {item.disposition === 'Conflict' && (
                    <span>
                      同名但内容不同，将使用上方新名称导入，已有文件保持不变。
                    </span>
                  )}
                  {item.dateUnknown && (
                    <span className="import-date-tag">拍摄日期未知</span>
                  )}
                  {item.warning && <em>{item.warning}</em>}
                </div>
                <span>{formatBytes(item.sizeBytes)}</span>
              </div>
            ))}
          </details>
        ))}
      </div>
      {Object.keys(draft.ignoredSuffixes).length > 0 && (
        <details className="import-details">
          <summary>查看忽略的其他文件</summary>
          <ul>
            {Object.entries(draft.ignoredSuffixes).map(([suffix, count]) => (
              <li key={suffix}>
                {suffix || '无扩展名'}：{count} 个
              </li>
            ))}
          </ul>
        </details>
      )}
    </section>
  );
}

function dispositionLabel(item: ImportDraftItem): string {
  if (item.disposition === 'Skip') return '跳过';
  if (item.disposition === 'Conflict') return '同名另存';
  if (item.disposition === 'Unreadable') return '无法读取';
  return '新增';
}
function Metric({ label, value }: { label: string; value: number }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>
        {value.toLocaleString('zh-CN')} <small>个文件</small>
      </dd>
    </div>
  );
}

export function ExecutionPanel({
  execution,
  recovery,
  stopping,
}: {
  execution: ImportExecutionStatus | null;
  recovery: ImportRecoverySummary | null;
  stopping: boolean;
}) {
  if (!execution)
    return (
      recovery && (
        <section className="import-progress-panel" aria-live="polite">
          <h1>发现上次未完成的导入</h1>
          <p>
            已完成 {recovery.completedFiles} 个文件，还有{' '}
            {recovery.unresolvedFiles}{' '}
            个需核对。选择继续后将重新检查来源和剩余文件。
          </p>
          {recovery.message && (
            <details className="import-details">
              <summary>查看任务详情</summary>
              <p>{recovery.message}</p>
            </details>
          )}
        </section>
      )
    );
  const active =
    execution.phase === 'running' || execution.phase === 'validatingPlan';
  const descriptions: Partial<Record<ImportExecutionStatus['phase'], string>> =
    {
      validatingPlan: '正在核对文件与保存位置，检查完成后继续导入。',
      running: '正在把计划中的新文件复制到图库。',
      awaitingPlanConfirmation:
        '来源或目标发生了变化。请检查下方更新后的文件名、保存位置和数量，再确认导入。',
      awaitingSpace: '释放目标磁盘空间后重新检查，或取消剩余导入。',
      awaitingSource:
        '请连接原来源磁盘，或重新选择原来源文件夹，然后继续未完成的文件。',
      needsAttention:
        '部分文件需要核对。查看下方异常项，处理后继续，或取消剩余导入。',
      completed: '本次导入已结束，可以返回图库继续浏览。',
      completedWithErrors:
        '已导入的照片保留在图库中。剩余项目可按下方操作继续处理。',
      cancelled:
        '已完成的照片保留在图库中，未完成的文件没有继续导入。保留的临时文件见下方详情。',
    };
  return (
    <section className="import-execution" aria-live="polite">
      <div className="import-heading">
        <h1>
          {stopping && active
            ? '正在停止导入…'
            : executionPhaseLabel(execution)}
        </h1>
        <p>
          {stopping && active
            ? '正在等待当前操作结束。已完成的照片会保留。'
            : descriptions[execution.phase]}
        </p>
      </div>
      {active && (
        <progress aria-label={stopping ? '正在停止导入' : '导入执行进度'} />
      )}
      <dl className="import-metrics">
        <Metric label="已完成处理" value={execution.completedFiles} />
        <Metric label="新增到图库" value={execution.publishedFiles} />
        <Metric label="已有相同，跳过" value={execution.skippedFiles} />
        <Metric label="需处理" value={execution.failedFiles} />
        <Metric label="未开始" value={execution.notStartedFiles} />
      </dl>
      <p className="import-muted">
        “已完成处理”是处理进度，其余数量分别描述文件结果，不与该项相加。
      </p>
      {active && execution.currentFile && (
        <p className="long-path">当前文件：{execution.currentFile}</p>
      )}
      {execution.space && (
        <div className="import-notice" role="status">
          <strong>还缺 {formatBytes(execution.space.missingBytes)}</strong>
          <p>
            剩余新增文件 {formatBytes(execution.space.plannedNewBytes)} · 可用{' '}
            {formatBytes(execution.space.availableBytes)} · 需预留{' '}
            {formatBytes(execution.space.reserveBytes)}
          </p>
        </div>
      )}
      {execution.phase === 'awaitingPlanConfirmation' &&
        execution.updatedPlan && (
          <DraftResults
            key={execution.updatedPlan.draftDigest}
            draft={execution.updatedPlan}
          />
        )}
      {execution.message && (
        <details className="import-details">
          <summary>查看处理详情</summary>
          <p>{execution.message}</p>
        </details>
      )}
    </section>
  );
}
