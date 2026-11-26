/**
 * Preparação do ambiente de teste.
 *
 * Os testes desta aplicação nunca falam com o Tauri de verdade: `invoke` e
 * `listen` são substituídos por dublês em cada arquivo de teste. O que sobra
 * aqui é o que vale para todos — os matchers de DOM e a desmontagem entre
 * casos, sem a qual um `getByText` acha o componente do teste anterior.
 */
import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';

afterEach(() => {
  cleanup();
});
