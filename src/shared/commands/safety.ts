import { invoke } from '@tauri-apps/api/core';

import { APP_VERSION } from '../version';

export type GateState = 'open' | 'closed';
export type DatabaseMode = 'read_write' | 'read_only_safe' | 'unavailable';
export type GateCloseReason =
  | 'startup'
  | 'database_read_only'
  | 'writer_unhealthy'
  | 'root_identity'
  | 'job_runner'
  | 'reconciler'
  | 'capability_boundary'
  | 'fault_latched';

export interface SafetyStatus {
  readonly phase: string;
  readonly globalWriteGate: GateState;
  readonly importGate: GateState;
  readonly recycleGate: GateState;
  readonly databaseMode: DatabaseMode;
  readonly reason: GateCloseReason | null;
  readonly message: string;
}

export async function getSafetyStatus(): Promise<SafetyStatus> {
  if (typeof window !== 'undefined' && !('__TAURI_INTERNALS__' in window)) {
    if (
      new URLSearchParams(window.location.search).get('safety') ===
      'database-readonly'
    ) {
      return {
        phase: `v${APP_VERSION}`,
        globalWriteGate: 'closed',
        importGate: 'closed',
        recycleGate: 'closed',
        databaseMode: 'read_only_safe',
        reason: 'database_read_only',
        message:
          '数据库由更新版本的应用创建；当前程序版本过旧，请安装最新版本后继续。',
      };
    }
    return {
      phase: `v${APP_VERSION}`,
      globalWriteGate: 'open',
      importGate: 'open',
      recycleGate: 'open',
      databaseMode: 'read_write',
      reason: null,
      message: '写入安全状态正常；Import 与 Recycle 可用。',
    };
  }
  return invoke<SafetyStatus>('get_safety_status');
}
