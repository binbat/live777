import { render } from 'preact';
import { App } from './app';
import '@/shared/tailwind.css';

render(<App />, document.getElementById('app')!);
