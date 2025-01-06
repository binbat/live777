import { render } from 'preact';

import '@/shared/tailwind.css';

import { Liveman } from './liveman';

render(<Liveman />, document.getElementById('app')!);
