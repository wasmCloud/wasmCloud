defmodule LoadProject do

  def load_actor(actor) do
    IO.puts("loading actor #{actor.name}")
    {:ok,bytes} = File.read(actor.path)
    IO.puts("loaded #{String.length(bytes)} bytes")
    HostCore.Actors.ActorSupervisor.start_actor(bytes)
  end

  def load_provider(provider) do
    IO.puts("loading provider #{provider.name} '${provider.contract}'")
    HostCore.Providers.ProviderSupervisor.start_executable_provider(
      provider.path,
      provider.key,
      provider.link,
      provider.contract)
  end

  def link(actor, provider) do
    HostCore.Linkdefs.Manager.put_link_definition(
      actor.key,
      provider.contract,
      provider.link,
      provider.key,
      provider.params)
  end

  def start(project) do

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

    # link (first) actor with all providers
    for provider <- project.providers do
        link(hd(project.actors),provider)
    end

  end
end
