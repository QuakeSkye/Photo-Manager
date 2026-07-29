import { render, screen } from '@testing-library/react';
import { beforeEach, expect, test, vi } from 'vitest';

import { App } from './App';

const { invokeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

beforeEach(() => {
  invokeMock.mockReset();
});

test('shows every photo-writing gate as closed', async () => {
  invokeMock.mockResolvedValue({
    phase: '阶段 0：工程与安全底座',
    globalWriteGate: 'closed',
    importGate: 'closed',
    recycleGate: 'closed',
    databaseMode: 'read_write',
    message: '安全底座已就绪。',
  });

  render(<App />);

  expect(await screen.findByText('安全底座已就绪。')).toBeInTheDocument();
  expect(screen.getAllByText('已关闭')).toHaveLength(3);
  expect(screen.getByText(/当前版本不会扫描照片/)).toBeInTheDocument();
});

test('fails closed when the backend status cannot be read', async () => {
  invokeMock.mockRejectedValue(new Error('backend unavailable'));

  render(<App />);

  expect(await screen.findByText(/照片写入功能保持关闭/)).toBeInTheDocument();
});
