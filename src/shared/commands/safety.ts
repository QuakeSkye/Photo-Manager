import { invoke } from '@tauri-apps/api/core';

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
  readonly phase: 'v1.0.1';
  readonly globalWriteGate: GateState;
  readonly importGate: GateState;
  readonly recycleGate: GateState;
  readonly databaseMode: DatabaseMode;
  readonly reason: GateCloseReason | null;
  readonly message: string;
}

export async function getSafetyStatus(): Promise<SafetyStatus> {
  if (typeof window !== 'undefined' && !('__TAURI_INTERNALS__' in window)) {
    return {
      phase: 'v1.0.1',
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
