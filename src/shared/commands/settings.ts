import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';

export interface PhotoshopStatus {
  readonly configured: boolean;
  readonly available: boolean;
  readonly toolId: string | null;
  readonly displayName: string;
  readonly executableLabel: string | null;
  readonly message: string;
}

export interface CacheSettings {
  readonly parentDisplayPath: string;
  readonly capacityBytes: number;
  readonly usedBytes: number;
  readonly availableBytes: number | null;
  readonly activeAvailable: boolean;
  readonly drainingRootCount: number;
  readonly drainingBytes: number;
}

export interface CacheLocationCandidate {
  readonly displayPath: string;
}

let demoPhotoshop: PhotoshopStatus = validDemoPhotoshop();
let demoCache: CacheSettings = {
  parentDisplayPath: 'C:\\Users\\摄影者\\AppData\\Local\\Fuji Photo Manager',
  capacityBytes: 32 * 1_073_741_824,
  usedBytes: 6_442_450_944,
  availableBytes: 187_904_819_200,
  activeAvailable: true,
  drainingRootCount: 0,
  drainingBytes: 0,
};

export async function getPhotoshopStatus(): Promise<PhotoshopStatus> {
  if (useDemoBackend()) {
    const scenario = currentScenario();
    if (scenario.includes('photoshop-unconfigured')) return unconfiguredDemo();
    if (scenario.includes('photoshop-invalid')) return invalidDemo();
    return demoPhotoshop;
  }
  return invoke<PhotoshopStatus>('get_photoshop_status');
}

export async function choosePhotoshop(): Promise<PhotoshopStatus | null> {
  if (useDemoBackend()) {
    if (currentScenario().includes('picker-cancel')) return null;
    demoPhotoshop = validDemoPhotoshop();
    return demoPhotoshop;
  }
  return invoke<PhotoshopStatus | null>('choose_photoshop');
}

export async function openRafInPhotoshop(
  toolId: string,
  photoId: string,
  photoVersion: number,
): Promise<void> {
  if (useDemoBackend()) return;
  await invoke('open_raf_in_photoshop', { toolId, photoId, photoVersion });
}

export async function openPhotoFileLocation(
  photoId: string,
  photoVersion: number,
): Promise<void> {
  if (useDemoBackend()) return;
  await invoke('open_photo_file_location', { photoId, photoVersion });
}

export async function getCacheSettings(): Promise<CacheSettings> {
  if (useDemoBackend()) {
    if (currentScenario().includes('cache-offline')) {
      return { ...demoCache, activeAvailable: false, availableBytes: null };
    }
    return demoCache;
  }
  return invoke<CacheSettings>('get_cache_settings');
}

export async function chooseCacheFolder(): Promise<string | null> {
  if (useDemoBackend()) return 'D:\\Fuji Preview Cache';
  const selected = await open({ directory: true, multiple: false });
  return typeof selected === 'string' ? selected : null;
}

export async function validateCacheLocation(
  path: string,
): Promise<CacheLocationCandidate> {
  if (useDemoBackend()) return { displayPath: path };
  return invoke<CacheLocationCandidate>('validate_cache_location', { path });
}

export async function updateCacheLocation(
  path: string,
  operationId: string,
): Promise<CacheSettings> {
  if (useDemoBackend()) {
    demoCache = {
      ...demoCache,
      parentDisplayPath: path,
      drainingRootCount: 1,
      drainingBytes: demoCache.usedBytes,
    };
    return demoCache;
  }
  return invoke<CacheSettings>('update_cache_location', { path, operationId });
}

export async function updateCacheCapacity(
  capacityBytes: number,
  operationId: string,
): Promise<CacheSettings> {
  if (useDemoBackend()) {
    demoCache = { ...demoCache, capacityBytes };
    return demoCache;
  }
  return invoke<CacheSettings>('update_cache_capacity', {
    capacityBytes,
    operationId,
  });
}

export function resetDemoSettings(): void {
  demoPhotoshop = validDemoPhotoshop();
  demoCache = {
    parentDisplayPath: 'C:\\Users\\摄影者\\AppData\\Local\\Fuji Photo Manager',
    capacityBytes: 32 * 1_073_741_824,
    usedBytes: 6_442_450_944,
    availableBytes: 187_904_819_200,
    activeAvailable: true,
    drainingRootCount: 0,
    drainingBytes: 0,
  };
}

function useDemoBackend(): boolean {
  return typeof window !== 'undefined' && !('__TAURI_INTERNALS__' in window);
}

function currentScenario(): string {
  return (
    new URLSearchParams(window.location.search).get('scenario') ?? 'library'
  );
}

function validDemoPhotoshop(): PhotoshopStatus {
  return {
    configured: true,
    available: true,
    toolId: '20000000-0000-4000-8000-000000000001',
    displayName: 'Adobe Photoshop',
    executableLabel: 'Photoshop.exe · 已验证',
    message: '可以把当前 RAF 交给 Photoshop。',
  };
}

function unconfiguredDemo(): PhotoshopStatus {
  return {
    configured: false,
    available: false,
    toolId: null,
    displayName: 'Adobe Photoshop',
    executableLabel: null,
    message: '尚未选择 Photoshop。',
  };
}

function invalidDemo(): PhotoshopStatus {
  return {
    configured: true,
    available: false,
    toolId: '20000000-0000-4000-8000-000000000001',
    displayName: 'Adobe Photoshop',
    executableLabel: 'Photoshop.exe · 需要重新选择',
    message: '已配置程序缺失或验证失败，请重新选择。',
  };
}
