import { invoke } from '@tauri-apps/api/core';

export type GateState = 'closed';
export type DatabaseMode = 'read_write' | 'read_only_safe' | 'unavailable';

export interface SafetyStatus {
  readonly phase: '阶段 0：工程与安全底座';
  readonly globalWriteGate: GateState;
  readonly importGate: GateState;
  readonly recycleGate: GateState;
  readonly databaseMode: DatabaseMode;
  readonly message: string;
}

export async function getSafetyStatus(): Promise<SafetyStatus> {
  return invoke<SafetyStatus>('get_safety_status');
}
