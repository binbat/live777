import { render } from 'preact';
import { App } from './App'; 
import '@/shared/tailwind.css';


render(<App />, document.getElementById('app')!);
