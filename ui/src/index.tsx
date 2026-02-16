/* @refresh reload */
import { render } from 'solid-js/web';
import { Router } from '@solidjs/router';
import './index.css';
import App from './App';

const root = document.getElementById('app');
if (!root) throw new Error('Root element #app not found');

render(() => <Router root={App} />, root);
