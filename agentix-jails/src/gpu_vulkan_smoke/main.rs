use anyhow::{ensure, Context, Result};
use ash::{vk, Entry};
use std::{ffi::CStr, io::Cursor};

fn main() -> Result<()> {
    // SAFETY: Load the Vulkan loader supplied by the Nix runtime environment.
    // Entry stays alive until after its instance and device are destroyed.
    let entry = unsafe { Entry::load() }.context("loading libvulkan.so.1")?;
    let application = vk::ApplicationInfo::default()
        .application_name(c"agentix-headless-compute")
        .api_version(vk::API_VERSION_1_0);
    // No surface, display, swapchain, window-system or debug extensions/layers.
    let create = vk::InstanceCreateInfo::default().application_info(&application);
    // SAFETY: All create-info pointers borrow live values; extension/layer counts
    // are zero. Objects, allocations and descriptor references below belong to
    // this single instance/device. Command recording/submission is single-threaded.
    // Output memory is HOST_VISIBLE|HOST_COHERENT; a shader-to-host barrier and
    // completed fence precede host reads. Cleanup occurs after completion; on an
    // early error this standalone process exits and the driver reclaims resources.
    unsafe {
        let instance = entry
            .create_instance(&create, None)
            .context("creating headless Vulkan instance")?;
        let mut selected = None;
        for physical in instance
            .enumerate_physical_devices()
            .context("enumerating physical devices")?
        {
            let properties = instance.get_physical_device_properties(physical);
            // Reject software Vulkan implementations: this is hardware acceptance.
            if !matches!(
                properties.device_type,
                vk::PhysicalDeviceType::DISCRETE_GPU | vk::PhysicalDeviceType::INTEGRATED_GPU
            ) {
                continue;
            }
            if let Some((index, _)) = instance
                .get_physical_device_queue_family_properties(physical)
                .iter()
                .enumerate()
                .find(|(_, queue)| {
                    queue.queue_count > 0 && queue.queue_flags.contains(vk::QueueFlags::COMPUTE)
                })
            {
                selected = Some((physical, properties, u32::try_from(index)?));
                break;
            }
        }
        let (physical, properties, family) = selected.context(
            "no hardware Vulkan device with a compute queue (software fallback refused)",
        )?;
        let name = CStr::from_ptr(properties.device_name.as_ptr()).to_string_lossy();
        println!(
            "Headless Vulkan: {name}, vendor 0x{:04x}, compute queue {family}",
            properties.vendor_id
        );
        let priorities = [1.0];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&priorities)];
        let device = instance
            .create_device(
                physical,
                &vk::DeviceCreateInfo::default().queue_create_infos(&queues),
                None,
            )
            .context("creating compute-only Vulkan device")?;
        let queue = device.get_device_queue(family, 0);
        let buffer = device.create_buffer(
            &vk::BufferCreateInfo::default()
                .size(4)
                .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
                .sharing_mode(vk::SharingMode::EXCLUSIVE),
            None,
        )?;
        let requirements = device.get_buffer_memory_requirements(buffer);
        let memory_properties = instance.get_physical_device_memory_properties(physical);
        let flags = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
        let memory_type = (0..memory_properties.memory_type_count)
            .find(|index| {
                requirements.memory_type_bits & (1 << index) != 0
                    && memory_properties.memory_types[*index as usize]
                        .property_flags
                        .contains(flags)
            })
            .context("no host-visible coherent memory type for output buffer")?;
        let memory = device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type),
            None,
        )?;
        device.bind_buffer_memory(buffer, memory, 0)?;
        let mapped = device
            .map_memory(memory, 0, 4, vk::MemoryMapFlags::empty())?
            .cast::<u32>();
        mapped.write(0);

        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)];
        let set_layout = device.create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
            None,
        )?;
        let layouts = [set_layout];
        let pipeline_layout = device.create_pipeline_layout(
            &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
            None,
        )?;
        let sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)];
        let descriptor_pool = device.create_descriptor_pool(
            &vk::DescriptorPoolCreateInfo::default()
                .pool_sizes(&sizes)
                .max_sets(1),
            None,
        )?;
        let sets = device.allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(descriptor_pool)
                .set_layouts(&layouts),
        )?;
        let set = *sets.first().context("allocated descriptor set")?;
        let buffer_info = [vk::DescriptorBufferInfo::default()
            .buffer(buffer)
            .offset(0)
            .range(4)];
        let writes = [vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&buffer_info)];
        device.update_descriptor_sets(&writes, &[]);

        let code = ash::util::read_spv(&mut Cursor::new(include_bytes!(env!(
            "AGENTIX_VULKAN_SMOKE_SHADER"
        ))))?;
        let shader = device
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None)?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(shader)
            .name(c"main");
        let pipelines = device
            .create_compute_pipelines(
                vk::PipelineCache::null(),
                &[vk::ComputePipelineCreateInfo::default()
                    .stage(stage)
                    .layout(pipeline_layout)],
                None,
            )
            .map_err(|(_, error)| anyhow::anyhow!("creating compute pipeline: {error:?}"))?;
        let pipeline = *pipelines.first().context("created compute pipeline")?;
        let command_pool = device.create_command_pool(
            &vk::CommandPoolCreateInfo::default().queue_family_index(family),
            None,
        )?;
        let commands = device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )?;
        let command = *commands.first().context("allocated command buffer")?;
        device.begin_command_buffer(
            command,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )?;
        // Order the host's initial zero before the shader's overwrite.
        let before = [vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::HOST_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_WRITE)];
        device.cmd_pipeline_barrier(
            command,
            vk::PipelineStageFlags::HOST,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::DependencyFlags::empty(),
            &before,
            &[],
            &[],
        );
        device.cmd_bind_pipeline(command, vk::PipelineBindPoint::COMPUTE, pipeline);
        device.cmd_bind_descriptor_sets(
            command,
            vk::PipelineBindPoint::COMPUTE,
            pipeline_layout,
            0,
            &[set],
            &[],
        );
        device.cmd_dispatch(command, 1, 1, 1);
        let after = [vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags::HOST_READ)];
        device.cmd_pipeline_barrier(
            command,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::HOST,
            vk::DependencyFlags::empty(),
            &after,
            &[],
            &[],
        );
        device.end_command_buffer(command)?;
        let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;
        let submits = [vk::SubmitInfo::default().command_buffers(&commands)];
        device.queue_submit(queue, &submits, fence)?;
        device
            .wait_for_fences(&[fence], true, 10_000_000_000)
            .context("waiting for compute completion (10s timeout)")?;
        let answer = mapped.read();
        ensure!(
            answer == 42,
            "incorrect Vulkan shader result: expected 42, got {answer}"
        );
        device.unmap_memory(memory);
        device.destroy_fence(fence, None);
        device.destroy_command_pool(command_pool, None);
        device.destroy_pipeline(pipeline, None);
        device.destroy_shader_module(shader, None);
        device.destroy_descriptor_pool(descriptor_pool, None);
        device.destroy_pipeline_layout(pipeline_layout, None);
        device.destroy_descriptor_set_layout(set_layout, None);
        device.destroy_buffer(buffer, None);
        device.free_memory(memory, None);
        device.destroy_device(None);
        instance.destroy_instance(None);
        println!("PASS: headless Vulkan compute on {name}, shader result {answer}; no display extensions");
    }
    Ok(())
}
