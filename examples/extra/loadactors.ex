defmodule LoadProject do

  def load_actor(actor) do
    IO.puts("loading actor #{actor.name}")
    {:ok,bytes} = File.read(actor.path)
    IO.puts("loaded #{String.length(bytes)} bytes")
    HostCore.Actors.ActorSupervisor.start_actor(bytes)
  end

  def load_provider(provider) do
    IO.puts("loading provider #{provider.name} '#{provider.contract}'")
    HostCore.Providers.ProviderSupervisor.start_executable_provider(
      provider.path,
      provider.key,
      provider.link,
      provider.contract)
  end

  def make_link(actor, provider, params) do
    IO.puts("linking actor #{actor.name} with provider #{provider.contract} params #{inspect(params)}")
    HostCore.Linkdefs.Manager.put_link_definition(
      actor.key,
      provider.contract,
      provider.link,
      provider.key,
     params)
  end

  # main entrypoint - initialize actor(s), provider(s), and link them
  def init(project) do

    for actor <- project.actors do
      load_actor(actor)
    end
    IO.puts("actors loaded")
    HostCore.Actors.ActorSupervisor.all_actors()

    for provider <- project.providers do
      load_provider(provider)
    end
    IO.puts("providers loaded")
    HostCore.Providers.ProviderSupervisor.all_providers()

    Process.sleep(3000)

    # make maps to lookup actor by name and provider by contract
    for link <- project.links do
      actor = Enum.find(project.actors, fn(item) ->
              item.name == link.actor end)
      if is_nil(actor) do
        raise "Invalid actor spec: no actor with name #{link.actor}"
      end
      provider = Enum.find(project.providers, fn(item) ->
               item.contract == link.contract end)
      if is_nil(provider) do
        raise "Invalid provider spec: no provider with contract #{link.contract}"
      end
      make_link(actor, provider, link.params)
    end
  end
end
