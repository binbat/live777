import { render } from 'preact';

import { Player } from './player.tsx';

import 'virtual:uno.css';
import './style.css';

render(<Player />, document.getElementById('app')!);
