class WebSocketClient {
  private _connection: WebSocket | null = null;

  /**
   * 根据location路径生成WebSocket地址，可传入自定义path
   * @param path WebSocket路径，默认 "/api/ws/image"
   * @returns {string} WebSocket地址
   */
  getWebSocketUrl(path: string = "/api/ws/image"): string {
    let protocol = "ws://";
    if (window.location.protocol === "https:") {
      protocol = "wss://";
    }
    return protocol + window.location.host + path;
  }

  /**
   * 连接WebSocket
   * @param params
   */
  connect(params: {
    path?: string; // 可选，自定义WebSocket路径
    onConnect: () => void;
    onData: (data: ArrayBuffer, tag?: string) => void;
    onClose: () => void;
    onError: (error: string) => void;
  }): void {
    if (!window.WebSocket) {
      params.onError("WebSocket Not Supported");
      return;
    }

    // 使用可选路径参数生成URL
    this._connection = new WebSocket(this.getWebSocketUrl(params.path));
    this._connection.binaryType = "arraybuffer";

    this._connection.onopen = () => {
      params.onConnect();
    };

    this._connection.onmessage = evt => {
      if (evt.data instanceof ArrayBuffer) {
        const { tag, payload } = this.decodeBinaryFrame(evt.data);
        params.onData(payload, tag);
      } else {
        console.warn("收到非二进制消息:", evt.data);
      }
    };

    this._connection.onclose = () => {
      params.onClose();
    };

    this._connection.onerror = err => {
      params.onError(err.toString());
    };
  }

  /**
   * 发送指令
   * @param {Object} params 指令参数（必须含有operate参数）
   */
  send(params: object): void {
    if (this._connection) {
      this._connection.send(JSON.stringify(params));
    }
  }

  /**
   * 发送普通操作指令
   * @param {String} data 操作指令
   */
  sendClientData(data: string): void {
    // 发送指令
    if (this._connection) {
      this._connection.send(
        JSON.stringify({ operate: "command", command: data })
      );
    }
  }
  /**
   * 关闭WebSocket连接
   */
  disconnect(): void {
    if (this._connection) {
      this._connection.close();
      this._connection = null;
    }
  }

  private decodeBinaryFrame(buffer: ArrayBuffer): {
    tag?: string;
    payload: ArrayBuffer;
  } {
    const view = new DataView(buffer);

    // 新协议：magic byte
    if (buffer.byteLength >= 5 && view.getUint8(0) === 0x01) {
      const tagLen = view.getInt32(1);

      if (tagLen > 0 && 5 + tagLen < buffer.byteLength) {
        const tagBytes = buffer.slice(5, 5 + tagLen);
        const tag = new TextDecoder().decode(tagBytes);
        const payload = buffer.slice(5 + tagLen);
        return { tag, payload };
      }
    }
    // 旧协议
    return { payload: buffer };
  }
}

export default WebSocketClient;
