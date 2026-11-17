package us.disasm.agentix.device

import android.annotation.SuppressLint
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothStatusCodes
import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.Looper
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import us.disasm.agentix.core.plaud.PlaudException
import us.disasm.agentix.core.plaud.PlaudTransport

@SuppressLint("MissingPermission")
@Suppress("DEPRECATION")
class PlaudGatt(private val context: Context) : PlaudTransport {
    private data class Signal(val operation: String, val status: Int, val value: Int = 0)
    private val signals = Channel<Signal>(8)
    private val notifications = Channel<ByteArray>(512)
    private val handler = Handler(Looper.getMainLooper())
    private var active: BluetoothGatt? = null
    private var writeCharacteristic: BluetoothGattCharacteristic? = null
    private var closed = false

    private fun fail(message: String) {
        val failure = PlaudException(message)
        signals.close(failure)
        notifications.close(failure)
    }

    private fun signal(gatt: BluetoothGatt, value: Signal) {
        if (closed || gatt !== active) return
        if (!signals.trySend(value).isSuccess) fail("Recorder operation queue overflow; reconnect to retry")
    }

    private fun notification(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic, bytes: ByteArray) {
        if (closed || gatt !== active || characteristic.uuid != uuid(0x2bb0)) return
        if (bytes.size > 514 || !notifications.trySend(bytes.copyOf()).isSuccess) {
            fail("Recorder sent data faster than it could be saved; retry to resume")
        }
    }

    private val callback = object : BluetoothGattCallback() {
        override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
            if (closed || gatt !== active) return
            if (status != BluetoothGatt.GATT_SUCCESS || newState == BluetoothProfile.STATE_DISCONNECTED) {
                fail("Plaud disconnected (Bluetooth status $status). Wake it and retry to resume.")
            } else if (newState == BluetoothProfile.STATE_CONNECTED) signal(gatt, Signal("connected", status))
        }

        override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) = signal(gatt, Signal("services", status))
        override fun onMtuChanged(gatt: BluetoothGatt, mtu: Int, status: Int) = signal(gatt, Signal("mtu", status, mtu))
        override fun onDescriptorWrite(gatt: BluetoothGatt, descriptor: BluetoothGattDescriptor, status: Int) {
            if (descriptor.uuid == uuid(0x2902) && descriptor.characteristic.uuid == uuid(0x2bb0)) signal(gatt, Signal("subscribed", status))
        }
        override fun onCharacteristicWrite(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic, status: Int) {
            if (characteristic.uuid == uuid(0x2bb1)) signal(gatt, Signal("written", status))
        }
        override fun onCharacteristicChanged(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic, value: ByteArray) =
            notification(gatt, characteristic, value)
        override fun onCharacteristicChanged(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic) {
            if (Build.VERSION.SDK_INT < 33) notification(gatt, characteristic, characteristic.value ?: byteArrayOf())
        }
    }

    suspend fun open(address: String): Unit = withContext(Dispatchers.Main.immediate) {
        check(active == null && !closed)
        val adapter = context.getSystemService(BluetoothManager::class.java)?.adapter
            ?: throw PlaudException("Bluetooth is unavailable")
        if (!adapter.isEnabled) throw PlaudException("Turn on Bluetooth, then retry")
        val device = adapter.getRemoteDevice(address)
        active = device.connectGatt(context, false, callback, BluetoothDevice.TRANSPORT_LE, BluetoothDevice.PHY_LE_1M_MASK, handler)
            ?: throw PlaudException("Could not open recorder connection")
        await("connected")
        val gatt = checkNotNull(active)
        if (!gatt.discoverServices()) throw PlaudException("Could not request recorder services")
        await("services")
        val service = gatt.getService(uuid(0x1910)) ?: throw PlaudException("Device does not expose the supported Plaud service")
        val notify = service.getCharacteristic(uuid(0x2bb0)) ?: throw PlaudException("Recorder notification characteristic missing")
        val write = service.getCharacteristic(uuid(0x2bb1)) ?: throw PlaudException("Recorder command characteristic missing")
        if (notify.properties and BluetoothGattCharacteristic.PROPERTY_NOTIFY == 0 ||
            write.properties and BluetoothGattCharacteristic.PROPERTY_WRITE == 0) throw PlaudException("Unsupported recorder GATT properties")
        writeCharacteristic = write
        if (!gatt.requestMtu(517)) throw PlaudException("Could not negotiate recorder packet size")
        if (await("mtu").value < 41) throw PlaudException("Recorder packet size is too small for the supported handshake")
        if (!gatt.setCharacteristicNotification(notify, true)) throw PlaudException("Could not enable recorder notifications")
        val descriptor = notify.getDescriptor(uuid(0x2902)) ?: throw PlaudException("Recorder notification descriptor missing")
        val accepted = if (Build.VERSION.SDK_INT >= 33) {
            gatt.writeDescriptor(descriptor, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE) == BluetoothStatusCodes.SUCCESS
        } else {
            descriptor.value = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
            gatt.writeDescriptor(descriptor)
        }
        if (!accepted) throw PlaudException("Could not subscribe to recorder notifications")
        await("subscribed")
        Unit
    }

    override suspend fun send(packet: ByteArray) = withContext(Dispatchers.Main.immediate) {
        val gatt = active ?: throw PlaudException("Recorder is disconnected")
        val characteristic = writeCharacteristic ?: throw PlaudException("Recorder is not ready")
        val accepted = if (Build.VERSION.SDK_INT >= 33) {
            gatt.writeCharacteristic(characteristic, packet, BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT) == BluetoothStatusCodes.SUCCESS
        } else {
            characteristic.writeType = BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT
            characteristic.value = packet
            gatt.writeCharacteristic(characteristic)
        }
        if (!accepted) throw PlaudException("Recorder command could not be sent")
        await("written")
        Unit
    }

    override suspend fun receive(): ByteArray = withTimeout(20_000) { notifications.receive() }

    private suspend fun await(operation: String): Signal = withTimeout(20_000) {
        val result = signals.receive()
        if (result.operation != operation || result.status != BluetoothGatt.GATT_SUCCESS) {
            throw PlaudException("Recorder $operation failed (Bluetooth status ${result.status})")
        }
        result
    }

    fun close() {
        check(Looper.myLooper() == Looper.getMainLooper())
        if (closed) return
        closed = true
        val gatt = active
        active = null
        writeCharacteristic = null
        signals.close()
        notifications.close()
        try { gatt?.disconnect() } catch (_: SecurityException) { /* Permission can be revoked during a transfer. */ }
        finally { gatt?.close() }
    }

    private fun uuid(short: Int): UUID = UUID.fromString("0000%04x-0000-1000-8000-00805f9b34fb".format(short))
}
