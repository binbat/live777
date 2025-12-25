#!/usr/bin/env node
/**
 * UDP Control Test Tool for live777 DataChannel Bridge (Node.js version)
 * 
 * Usage:
 *   node tests/udp_control_test.js --host 127.0.0.1 --port 5005 --message "test"
 */

const dgram = require('dgram');
const readline = require('readline');

class UdpControlTester {
  constructor(host = '127.0.0.1', port = 5005) {
    this.host = host;
    this.port = port;
    this.client = dgram.createSocket('udp4');
  }

  sendMessage(message, callback) {
    const buffer = Buffer.from(message);
    this.client.send(buffer, this.port, this.host, (err) => {
      if (err) {
        console.error(`✗ Error sending message: ${err.message}`);
      } else {
        console.log(`✓ Sent message: ${message}`);
      }
      if (callback) callback(err);
    });
  }

  sendJson(data, callback) {
    const message = JSON.stringify(data);
    this.sendMessage(message, callback);
  }

  sendBinary(hexString, callback) {
    const buffer = Buffer.from(hexString, 'hex');
    this.client.send(buffer, this.port, this.host, (err) => {
      if (err) {
        console.error(`✗ Error sending binary: ${err.message}`);
      } else {
        console.log(`✓ Sent binary: ${hexString}`);
      }
      if (callback) callback(err);
    });
  }

  sendPtzCommand(action, params = {}, callback) {
    const command = { action, ...params };
    this.sendJson(command, callback);
  }

  async stressTest(count, interval = 10) {
    console.log(`\n=== Stress Test ===`);
    console.log(`Sending ${count} messages to ${this.host}:${this.port}`);
    console.log(`Interval: ${interval}ms\n`);

    let success = 0;
    let failed = 0;

    for (let i = 0; i < count; i++) {
      const message = JSON.stringify({
        seq: i,
        timestamp: Date.now()
      });

      await new Promise((resolve) => {
        this.sendMessage(message, (err) => {
          if (err) {
            failed++;
          } else {
            success++;
          }
          
          if ((i + 1) % 100 === 0) {
            console.log(`Sent ${i + 1}/${count} messages...`);
          }

          setTimeout(resolve, interval);
        });
      });
    }

    console.log(`\n=== Results ===`);
    console.log(`Success: ${success}`);
    console.log(`Failed: ${failed}`);
    console.log(`Success rate: ${(success / count * 100).toFixed(2)}%`);
  }

  interactiveMode() {
    console.log(`\n=== UDP Control Interactive Mode ===`);
    console.log(`Target: ${this.host}:${this.port}`);
    console.log(`Commands:`);
    console.log(`  text <message>     - Send text message`);
    console.log(`  json <json_string> - Send JSON message`);
    console.log(`  pan <left|right>   - Pan camera`);
    console.log(`  tilt <up|down>     - Tilt camera`);
    console.log(`  zoom <in|out>      - Zoom camera`);
    console.log(`  quit               - Exit`);
    console.log();

    const rl = readline.createInterface({
      input: process.stdin,
      output: process.stdout,
      prompt: 'udp> '
    });

    rl.prompt();

    rl.on('line', (line) => {
      const input = line.trim();
      if (!input) {
        rl.prompt();
        return;
      }

      const parts = input.split(/\s+/);
      const command = parts[0].toLowerCase();
      const args = parts.slice(1);

      switch (command) {
        case 'quit':
        case 'exit':
          console.log('Exiting...');
          rl.close();
          this.client.close();
          return;

        case 'text':
          if (args.length > 0) {
            this.sendMessage(args.join(' '));
          } else {
            console.log('✗ Usage: text <message>');
          }
          break;

        case 'json':
          if (args.length > 0) {
            try {
              const data = JSON.parse(args.join(' '));
              this.sendJson(data);
            } catch (e) {
              console.log(`✗ Invalid JSON: ${e.message}`);
            }
          } else {
            console.log('✗ Usage: json <json_string>');
          }
          break;

        case 'pan':
          if (args.length > 0 && ['left', 'right'].includes(args[0])) {
            this.sendPtzCommand('pan', { direction: args[0], speed: 50 });
          } else {
            console.log('✗ Usage: pan <left|right>');
          }
          break;

        case 'tilt':
          if (args.length > 0 && ['up', 'down'].includes(args[0])) {
            this.sendPtzCommand('tilt', { direction: args[0], speed: 50 });
          } else {
            console.log('✗ Usage: tilt <up|down>');
          }
          break;

        case 'zoom':
          if (args.length > 0 && ['in', 'out'].includes(args[0])) {
            this.sendPtzCommand('zoom', { direction: args[0], value: 1 });
          } else {
            console.log('✗ Usage: zoom <in|out>');
          }
          break;

        default:
          console.log(`✗ Unknown command: ${command}`);
      }

      rl.prompt();
    });

    rl.on('close', () => {
      this.client.close();
      process.exit(0);
    });
  }

  close() {
    this.client.close();
  }
}

// Parse command line arguments
function parseArgs() {
  const args = process.argv.slice(2);
  const options = {
    host: '127.0.0.1',
    port: 5005,
    message: null,
    json: null,
    binary: null,
    interactive: false,
    stress: null,
    interval: 10
  };

  for (let i = 0; i < args.length; i++) {
    switch (args[i]) {
      case '--host':
        options.host = args[++i];
        break;
      case '--port':
        options.port = parseInt(args[++i]);
        break;
      case '--message':
        options.message = args[++i];
        break;
      case '--json':
        options.json = args[++i];
        break;
      case '--binary':
        options.binary = args[++i];
        break;
      case '--interactive':
        options.interactive = true;
        break;
      case '--stress':
        options.stress = parseInt(args[++i]);
        break;
      case '--interval':
        options.interval = parseFloat(args[++i]);
        break;
      case '--help':
        console.log('UDP Control Test Tool');
        console.log('\nOptions:');
        console.log('  --host <host>       Target host (default: 127.0.0.1)');
        console.log('  --port <port>       Target port (default: 5005)');
        console.log('  --message <text>    Send a text message');
        console.log('  --json <json>       Send a JSON message');
        console.log('  --binary <hex>      Send binary data (hex string)');
        console.log('  --interactive       Enter interactive mode');
        console.log('  --stress <count>    Stress test with COUNT messages');
        console.log('  --interval <ms>     Interval between messages (default: 10ms)');
        console.log('  --help              Show this help');
        process.exit(0);
    }
  }

  return options;
}

async function main() {
  const options = parseArgs();
  const tester = new UdpControlTester(options.host, options.port);

  if (options.message) {
    tester.sendMessage(options.message, () => tester.close());
  } else if (options.json) {
    try {
      const data = JSON.parse(options.json);
      tester.sendJson(data, () => tester.close());
    } catch (e) {
      console.error(`✗ Invalid JSON: ${e.message}`);
      tester.close();
      process.exit(1);
    }
  } else if (options.binary) {
    tester.sendBinary(options.binary, () => tester.close());
  } else if (options.stress) {
    await tester.stressTest(options.stress, options.interval);
    tester.close();
  } else if (options.interactive) {
    tester.interactiveMode();
  } else {
    console.log('UDP Control Test Tool');
    console.log('\nExamples:');
    console.log(`  node ${process.argv[1]} --message "Hello"`);
    console.log(`  node ${process.argv[1]} --json '{"action":"pan","direction":"left"}'`);
    console.log(`  node ${process.argv[1]} --binary "010032"`);
    console.log(`  node ${process.argv[1]} --interactive`);
    console.log(`  node ${process.argv[1]} --stress 1000 --interval 10`);
    console.log('\nUse --help for more options');
    tester.close();
  }
}

main().catch((err) => {
  console.error('Error:', err);
  process.exit(1);
});
