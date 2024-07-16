import { render } from 'preact';

import 'virtual:uno.css';
import '../shared/index.css';
import { Liveion } from './liveion';

render(<Liveion />, document.getElementById('app')!);
