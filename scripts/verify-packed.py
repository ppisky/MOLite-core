"""Exercise the actual packaged Windows DLL and server, without third-party Python modules."""
import base64
import ctypes as c
import json
import os
from pathlib import Path
import subprocess
import struct
import sys
import tempfile
import time
import urllib.request


class Buffer(c.Structure):
    _fields_ = [("data", c.POINTER(c.c_ubyte)), ("length", c.c_size_t)]


def verify(directory, reference=None):
    if reference is not None:
        for name in ["molite_core.dll", "molite-server.exe"]:
            def attributes(path):
                data = path.read_bytes()
                offset = struct.unpack_from("<I", data, 60)[0] + 24
                assert struct.unpack_from("<H", data, offset)[0] == 0x20B, "expected PE32+"
                flags = struct.unpack_from("<H", data, offset + 70)[0]
                reloc = struct.unpack_from("<II", data, offset + 112 + 5 * 8)
                return flags, reloc
            original_flags, original_reloc = attributes(reference / name)
            packed_flags, packed_reloc = attributes(directory / name)
            assert original_flags == packed_flags, "PE DLL characteristics changed"
            if original_reloc[1]:
                assert packed_reloc[0] and packed_reloc[1], "relocation directory missing"
        print("PE: DLL characteristics (including ASLR/DEP) retained; relocations present", flush=True)
    dll = c.CDLL(str(directory / ("molite_core.dll" if os.name == "nt" else ("libmolite_core.dylib" if sys.platform == "darwin" else "libmolite_core.so"))))
    dll.molite_open.argtypes = [c.c_char_p, c.c_size_t, c.POINTER(c.c_uint64)]
    dll.molite_submit.argtypes = [c.c_uint64, c.c_char_p, c.c_size_t, c.POINTER(c.c_uint64)]
    dll.molite_task_next.argtypes = [c.c_uint64, c.c_uint32, c.POINTER(Buffer)]
    dll.molite_buffer_free.argtypes = [Buffer]
    dll.molite_buffer_free.restype = None
    for name in ["molite_close", "molite_task_release", "molite_task_cancel"]:
        getattr(dll, name).argtypes = [c.c_uint64]
    assert dll.molite_abi_version() == 1

    def execute(engine, request):
        payload = json.dumps(request).encode()
        task = c.c_uint64()
        assert dll.molite_submit(engine, payload, len(payload), c.byref(task)) == 0
        try:
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                buffer = Buffer()
                status = dll.molite_task_next(task, 1000, c.byref(buffer))
                if status == 1:
                    continue
                assert status == 0, status
                try:
                    event = json.loads(c.string_at(buffer.data, buffer.length))
                finally:
                    dll.molite_buffer_free(buffer)
                if event["type"] == "failed":
                    raise AssertionError(event)
                if event["type"] == "completed":
                    return event["result"]
            raise TimeoutError("ABI operation timed out")
        finally:
            assert dll.molite_task_release(task) == 0

    with tempfile.TemporaryDirectory(prefix="molite-packed-") as temporary:
        root = Path(temporary)
        archive = root / "encrypted.moc"
        payload = base64.b64encode("MOLite 数据往返".encode()).decode()
        file_id = None
        for index in range(2):
            engine = c.c_uint64()
            path = str(root / str(index)).encode()
            assert dll.molite_open(path, len(path), c.byref(engine)) == 0
            try:
                if index == 0:
                    execute(engine, {"op": "conversation.put", "value": {"id": "packed", "title": "Packed conversation"}})
                    execute(engine, {"op": "prompt_spaces.replace", "id": "assistant", "content": "Local prompt override"})
                    info = execute(engine, {"op": "file.add", "name": "data.txt", "mime": "text/plain", "base64": payload})
                    file_id = info["id"]
                    execute(engine, {"op": "moc.export", "path": str(archive), "passphrase": "local-test-passphrase"})
                else:
                    execute(engine, {"op": "moc.import", "path": str(archive), "passphrase": "local-test-passphrase"})
                    assert execute(engine, {"op": "file.read", "id": file_id})["base64"] == payload
                conversations = execute(engine, {"op": "resource.list", "kind": "conversation"})
                assert len(conversations) == 1 and conversations[0]["id"] == "packed"
                assert "scene_id" not in conversations[0]
                assert execute(engine, {"op": "prompt_spaces.get", "id": "assistant"})["source"] == ("override" if index == 0 else "builtin")
            finally:
                assert dll.molite_close(engine) == 0
        print("DLL: ABI lifecycle, SQLite, encrypted MOC/file roundtrip passed", flush=True)

        env = dict(os.environ, MOLITE_DATA=str(root / "http"), MOLITE_LISTEN="127.0.0.1:0", MOLITE_HTTP_TOKEN="packed-test")
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with (root / "server.log").open("w+b") as log:
            process = subprocess.Popen([str(directory / ("molite-server.exe" if os.name == "nt" else "molite-server"))], env=env,
                                       stdout=subprocess.DEVNULL, stderr=log,
                                       creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
            try:
                address = None
                deadline = time.monotonic() + 30
                while time.monotonic() < deadline:
                    assert process.poll() is None, "server exited before listening"
                    log.seek(0)
                    message = log.read().decode(errors="replace")
                    if "MOLite listening on " in message:
                        address = message.split("MOLite listening on ")[1].splitlines()[0]
                        break
                    time.sleep(0.05)
                assert address, "server did not listen"
                base = "http://" + address
                with opener.open(base + "/health", timeout=10) as response:
                    assert json.load(response)["status"] == "ok"
                request = urllib.request.Request(base + "/v1/prompt-spaces", headers={"Authorization": "Bearer packed-test"})
                with opener.open(request, timeout=10) as response:
                    assert len(json.load(response)) == 4
                for route in ["operations", "stream"]:
                    request = urllib.request.Request(base + "/v1/" + route,
                        data=b'{"op":"resource.list","kind":"conversation"}',
                        headers={"Content-Type": "application/json", "Authorization": "Bearer packed-test"})
                    with opener.open(request, timeout=10) as response:
                        body = response.read().decode()
                        if route == "operations":
                            assert json.loads(body) == []
                        else:
                            assert "event: completed" in body and '"result":[]' in body, body
                print("EXE: HTTP health, authenticated operation and SSE passed", flush=True)
            finally:
                if process.poll() is None:
                    process.terminate()
                process.wait(timeout=10)


if __name__ == "__main__":
    verify(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve() if len(sys.argv) > 2 else None)
