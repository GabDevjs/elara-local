/**
 * Casca da janela: navegação, faixa de estado e a tela ativa.
 *
 * A rota é um estado local porque são duas telas. Trazer um roteador para cá
 * adicionaria histórico, parsing de URL e um `<Suspense>` para resolver um
 * problema que a janela não tem — ela não tem barra de endereço.
 */
import { useState, type ReactNode } from 'react';

import { NavRail } from './components/NavRail.js';
import { StatusStrip } from './components/StatusStrip.js';
import { DaemonProvider } from './lib/daemon-session.js';
import type { RouteId } from './lib/navigation.js';
import { Dashboard } from './routes/Dashboard.js';
import { Hardware } from './routes/Hardware.js';

/** Raiz da aplicação. */
export function App(): ReactNode {
  const [route, setRoute] = useState<RouteId>('dashboard');

  return (
    <DaemonProvider>
      <div className="flex h-full min-h-0 bg-void">
        <NavRail current={route} onNavigate={setRoute} />
        <div className="flex min-w-0 flex-1 flex-col">
          <StatusStrip />
          <main className="flex min-h-0 flex-1 flex-col">
            {route === 'dashboard' ? <Dashboard /> : <Hardware />}
          </main>
        </div>
      </div>
    </DaemonProvider>
  );
}
