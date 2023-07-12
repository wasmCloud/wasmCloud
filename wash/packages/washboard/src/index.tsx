import './index.css';
import * as React from 'react';
import * as ReactDOM from 'react-dom/client';
import App from './App.tsx';

ReactDOM.createRoot(document.querySelector('#root') as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
