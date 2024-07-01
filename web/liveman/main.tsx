import { render } from 'preact'

import 'virtual:uno.css'
import '../shared/index.css'
import { Liveman } from './liveman'

render(<Liveman />, document.getElementById('app')!)
