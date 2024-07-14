import { render } from 'preact';

import './debugger.js';
import DebuggerCompat from './compat.tsx';

import './debugger.css';

render(<DebuggerCompat />, document.getElementById('app')!);
