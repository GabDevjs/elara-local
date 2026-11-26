/**
 * Ponto de entrada do webview.
 *
 * A janela é montada mesmo com o daemon fora do ar: quem trata a ausência é a
 * própria interface, com uma tela que diz onde está o socket e como subir o
 * daemon. Uma janela em branco esperando conexão seria indistinguível de um
 * travamento.
 */
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { App } from './App.js';
import './styles/theme.css';

const container = document.getElementById('root');

if (container === null) {
  // Só acontece se o `index.html` for editado sem o nó raiz: falhar alto aqui
  // é melhor do que uma tela preta sem explicação.
  throw new Error('elemento #root não encontrado no index.html');
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
