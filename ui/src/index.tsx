/* @refresh reload */
import { render } from 'solid-js/web';
import { HashRouter, Route } from '@solidjs/router';
import './index.css';
import App from './App';
import BatchesPage from './pages/BatchesPage';
import DagPage from './pages/DagPage';

// Legacy redirect: /view?batch=abc → #/batch/abc
const url = new URL(window.location.href);
const legacyBatch = url.searchParams.get('batch');
if (legacyBatch && !window.location.hash) {
  url.searchParams.delete('batch');
  window.history.replaceState(null, '', `${url.pathname}${url.search}`);
  window.location.hash = `/batch/${legacyBatch}`;
}

const root = document.getElementById('app');
if (!root) throw new Error('Root element #app not found');

render(
  () => (
    <HashRouter root={App}>
      <Route path="/" component={BatchesPage} />
      <Route path="/batch/:batchId" component={DagPage} />
    </HashRouter>
  ),
  root,
);
