# AudioSender

**Ultra-low latency audio streaming between Windows computers**

AudioSender is a lightweight audio relay application designed for streaming PC audio to another PC with minimal latency. Built with Rust for maximum performance, it's perfect for users who want simple, fast computer-to-computer audio transfer over a local network.

## 🚀 Features

- **Ultra-Low Latency**: Built for real-time audio streaming with minimal delay
- **Auto-Discovery**: Clients automatically find servers on the local network
- **WASAPI Integration**: Uses Windows Audio Session API for high-quality audio capture
- **Opus Codec**: Low-latency audio compression at 128 kbps
- **UDP Streaming**: Prioritizes real-time delivery over guaranteed delivery
- **CLI Interface**: Simple command-line operation

## 📋 Requirements

- Windows 10 or newer
- Same local network for auto-discovery (or direct IP connection)
- Firewall configured to allow UDP ports 59100 (discovery) and 59200 (streaming)

## 🔧 Installation

The application is compiled and ready to use. The executable is located at:

```
target\release\audio_sender.exe
```

## 💻 Usage

### Server Mode (Computer with audio source)

Run this on the computer whose audio you want to stream:

```bash
audio_sender.exe server
```

The server will:
- Start capturing system audio (loopback)
- Broadcast its presence on the network
- Wait for client connections

### Client Mode (Computer to receive audio)

Run this on the computer where you want to hear the audio:

```bash
# Auto-discover and connect to server
audio_sender.exe client

# Or connect directly to a specific IP
audio_sender.exe client --connect 192.168.1.100
```

The client will:
- Discover available servers (or connect to specified IP)
- Stream audio and play it through your speakers/headphones

## 🔍 How It Works

1. **Server** captures system audio using WASAPI loopback
2. Audio is encoded with **Opus codec** (low latency mode)
3. Packets are sent over **UDP** for minimal overhead
4. **Client** receives packets, decodes, and plays the audio
5. **Auto-discovery** lets clients find servers automatically via UDP broadcast

## ⚙️ Configuration

Configuration is stored at: `%APPDATA%\AudioSender\config.toml`

Default settings:
- Buffer size: 5ms
- Bitrate: 128 kbps
- Sample rate: 48 kHz
- Channels: Stereo (2)

## 🔥 Firewall Configuration

You may need to allow AudioSender through Windows Firewall:

1. Run the server - Windows will prompt for firewall access
2. Click "Allow access" for both Private and Public networks
3. Alternatively, add manual rules for:
   - UDP port **59100** (discovery)
   - UDP port **59200** (streaming)

## 🐛 Troubleshooting

**Client can't find server:**
- Ensure both computers are on the same network
- Check Windows Firewall settings
- Try direct IP connection: `audio_sender.exe client --connect <SERVER_IP>`

**Audio is choppy/glitchy:**
- Check network quality (ping between computers)
- Reduce other network traffic
- Ensure no CPU bottlenecks

**No audio output:**
- Verify correct audio device is selected in Windows
- Check volume levels on client computer
- Restart both server and client

## 📝 Technical Details

- **Language**: Rust
- **Audio API**: WASAPI (Windows Audio Session API)
- **Codec**: Opus (LOW_DELAY mode)
- **Network**: UDP for streaming, UDP broadcast for discovery
- **Frame Size**: 5ms (240 samples at 48kHz)
- **Packet Format**: Sequence number + Timestamp + Opus payload

## 🚧 Roadmap

Future improvements (not yet implemented):
- [ ] Graphical user interface
- [ ] Windows startup integration
- [ ] System tray icon
- [ ] Multiple client support
- [ ] Quality presets (latency vs stability)
- [ ] Audio device selection

## 📄 License

This project is open source.

## 🤝 Contributing

Contributions are welcome! Feel free to submit issues or pull requests.

---

**Note**: This application is designed for local network use. For best performance, use a wired Ethernet connection between computers.
