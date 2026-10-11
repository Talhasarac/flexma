import '@fontsource-variable/inter';
import './styles.css';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import { applyTheme } from './lib/theme';

// Apply the saved light/dark preference before the first paint.
applyTheme();

const root = document.getElementById('root');
if (!root) throw new Error('Missing #root element');

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
